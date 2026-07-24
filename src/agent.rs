//! Interactive review-first AI agent built on the shared `jagent` core.
//!
//! The model may only propose commands; every proposal goes through an
//! explicit approval prompt (or the opt-in read-only auto-approval allowlist)
//! before rsh executes it. Approved commands run in a forked child through the
//! normal rsh parser/executor with stdout+stderr teed to the terminal and
//! captured as the bounded observation for the next model turn.
//!
//! Configuration reuses the `RSH_AI_*` environment contract from `crate::ai`,
//! plus:
//! - `RSH_AGENT_MAX_TURNS` — model-turn budget (default 16)
//! - `RSH_AGENT_AUTO_APPROVE_READONLY` — auto-run commands accepted by
//!   `jagent::is_auto_approvable` (fail-closed allowlist; everything else
//!   still prompts)

use crate::ai::{AiConfig, AiProvider};
use crate::environment::ShellState;
use jagent::provider::{build_chat_request, parse_chat_response, ChatConfig, Message};
use jagent::{
    AgentSession, AgentState, ApprovedCommand, EnvironmentMeta, GitMeta, ModelOutcome, Provider,
    Role, SessionError,
};
use std::io::{IsTerminal, Write};

const DEFAULT_MAX_TURNS: u32 = 16;
const AGENT_MAX_TOKENS: u32 = 1024;
/// Collect at most this much execution output; `AgentSession::observe` samples
/// it further down to its own observation budget.
const MAX_CAPTURED_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_CONSECUTIVE_PROTOCOL_RETRIES: u32 = 2;

pub fn builtin_agent(args: &[String], state: &mut ShellState) -> i32 {
    let Some(ai_config) = AiConfig::from_env() else {
        eprintln!(
            "agent: AI is not configured. Set RSH_AI_PROVIDER=anthropic|openai|ollama \
             (plus the provider API key) or RSH_AI_ENABLED=1; see README."
        );
        return 1;
    };
    let chat = chat_config(&ai_config);
    let share_context = ai_config.allows_extended_context();

    let goal = if args.is_empty() {
        match read_line("agent goal> ") {
            Some(line) if !line.trim().is_empty() => line,
            _ => return 0,
        }
    } else {
        args.join(" ")
    };

    let mut session = AgentSession::new(max_turns());
    if let Err(error) = session.submit_user(goal) {
        eprintln!("agent: {error}");
        return 1;
    }

    let auto_readonly = env_truthy("RSH_AGENT_AUTO_APPROVE_READONLY");
    let mut protocol_retries = 0_u32;

    loop {
        match session.state() {
            AgentState::AwaitingModel => {
                status_line(&format!(
                    "thinking… (turn {}/{})",
                    session.turns_used() + 1,
                    session.max_turns()
                ));
                let reply = match request_model(&chat, &session, share_context) {
                    Ok(reply) => reply,
                    Err(error) => {
                        let _ = session.model_failed(&error);
                        eprintln!("agent: model request failed: {error}");
                        if session.can_retry_model() && confirm("retry? [y/N] ") {
                            let _ = session.retry_model();
                            continue;
                        }
                        return 1;
                    }
                };
                match session.accept_model_reply(&reply) {
                    Ok(ModelOutcome::Proposal {
                        id,
                        command,
                        danger,
                    }) => {
                        protocol_retries = 0;
                        let approved = match review_proposal(
                            &mut session,
                            id,
                            &command,
                            danger,
                            auto_readonly,
                        ) {
                            ReviewOutcome::Approved(approved) => approved,
                            ReviewOutcome::Rejected => continue,
                            ReviewOutcome::Quit => return 0,
                        };
                        let (exit_code, output) = run_captured(&approved.command, state);
                        if let Err(error) =
                            session.observe(approved.proposal_id, exit_code, &output)
                        {
                            eprintln!("agent: {error}");
                            return 1;
                        }
                    }
                    Ok(ModelOutcome::Said(message)) => {
                        protocol_retries = 0;
                        println!("agent: {message}");
                    }
                    Ok(ModelOutcome::Completed(message)) => {
                        protocol_retries = 0;
                        println!("agent done: {message}");
                    }
                    Err(SessionError::Protocol(error)) => {
                        eprintln!("agent: model reply violated the protocol: {error}");
                        if protocol_retries < MAX_CONSECUTIVE_PROTOCOL_RETRIES
                            && session.can_retry_model()
                        {
                            protocol_retries += 1;
                            let _ = session.retry_model();
                        }
                    }
                    Err(error) => {
                        eprintln!("agent: {error}");
                        return 1;
                    }
                }
            }
            AgentState::Ready => {
                let Some(line) = read_line("you> ") else {
                    return 0;
                };
                let line = line.trim().to_string();
                if line.is_empty() || line == "q" || line == "quit" {
                    return 0;
                }
                if let Err(error) = session.submit_user(line) {
                    eprintln!("agent: {error}");
                    if matches!(error, SessionError::TurnLimitReached) {
                        return 1;
                    }
                }
            }
            AgentState::Completed => {
                if !session.can_continue_after_completion() {
                    return 0;
                }
                let Some(line) = read_line("follow-up (Enter to finish)> ") else {
                    return 0;
                };
                let line = line.trim().to_string();
                if line.is_empty() {
                    return 0;
                }
                if session.continue_after_completion().is_err() {
                    return 0;
                }
                if let Err(error) = session.submit_user(line) {
                    eprintln!("agent: {error}");
                    return 1;
                }
            }
            AgentState::TurnLimitReached => {
                eprintln!(
                    "agent: turn budget of {} reached (RSH_AGENT_MAX_TURNS to raise)",
                    session.max_turns()
                );
                return 1;
            }
            AgentState::Cancelled => return 0,
            AgentState::AwaitingApproval { .. } | AgentState::AwaitingObservation { .. } => {
                // Both are resolved inline in the proposal arm; reaching here
                // means an internal bug, so fail instead of spinning.
                eprintln!("agent: internal state error");
                return 1;
            }
        }
    }
}

enum ReviewOutcome {
    Approved(ApprovedCommand),
    Rejected,
    Quit,
}

fn review_proposal(
    session: &mut AgentSession,
    id: jagent::ProposalId,
    command: &str,
    danger: Option<&'static str>,
    auto_readonly: bool,
) -> ReviewOutcome {
    println!();
    println!("  proposed: {}", emphasize(command));
    if let Some(reason) = danger {
        println!("  {}", warn(&format!("warning: {reason}")));
    }
    if auto_readonly && danger.is_none() && jagent::is_auto_approvable(command) {
        println!("  auto-approved (read-only allowlist)");
        match session.approve(id) {
            Ok(approved) => return ReviewOutcome::Approved(approved),
            Err(error) => {
                eprintln!("agent: {error}");
                return ReviewOutcome::Quit;
            }
        }
    }
    loop {
        let Some(choice) = read_line("  [y] run  [e] edit  [n] reject  [q] quit > ") else {
            session.cancel();
            return ReviewOutcome::Quit;
        };
        match choice.trim() {
            "y" | "yes" => {
                let approved = match session.approve(id) {
                    Ok(approved) => approved,
                    Err(error) => {
                        eprintln!("agent: {error}");
                        return ReviewOutcome::Quit;
                    }
                };
                if !confirm_danger(&approved) {
                    // The state machine has already recorded the approval, so
                    // backing out means ending the session rather than
                    // pretending the proposal is pending again.
                    session.cancel();
                    return ReviewOutcome::Quit;
                }
                return ReviewOutcome::Approved(approved);
            }
            "e" | "edit" => {
                let Some(edited) = read_line("  edit> ") else {
                    continue;
                };
                match session.edit_and_approve(id, edited) {
                    Ok(approved) => {
                        if !confirm_danger(&approved) {
                            session.cancel();
                            return ReviewOutcome::Quit;
                        }
                        return ReviewOutcome::Approved(approved);
                    }
                    Err(error) => {
                        eprintln!("  agent: {error}");
                    }
                }
            }
            "n" | "no" | "reject" => {
                if let Err(error) = session.reject(id) {
                    eprintln!("agent: {error}");
                    return ReviewOutcome::Quit;
                }
                return ReviewOutcome::Rejected;
            }
            "q" | "quit" => {
                session.cancel();
                return ReviewOutcome::Quit;
            }
            _ => {}
        }
    }
}

/// Recognized-dangerous commands need a second, deliberate confirmation after
/// approval, mirroring jterm4's exact-command confirmation gate.
fn confirm_danger(approved: &ApprovedCommand) -> bool {
    let Some(reason) = approved.danger else {
        return true;
    };
    println!("  {}", warn(&format!("dangerous: {reason}")));
    match read_line("  type RUN to execute, anything else aborts > ") {
        Some(line) => line.trim() == "RUN",
        None => false,
    }
}

fn request_model(
    chat: &ChatConfig,
    session: &AgentSession,
    share_context: bool,
) -> Result<String, String> {
    let environment = environment_meta(share_context);
    let user_text = jagent::agent_user_prompt(&session.build_user_prompt(), &environment, None);
    let request = build_chat_request(
        chat,
        Some(&jagent::build_agent_system_prompt()),
        &[Message {
            role: Role::User,
            text: user_text,
        }],
    )
    .map_err(|error| error.to_string())?;

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(120))
        .timeout_write(std::time::Duration::from_secs(10))
        .build();
    let mut post = agent.post(&request.url);
    for (name, value) in &request.headers {
        post = post.set(name, value);
    }
    let response = post
        .send_string(&request.body)
        .map_err(|error| match error {
            ureq::Error::Status(status, response) => format!(
                "HTTP {status}: {}",
                response.into_string().unwrap_or_default()
            ),
            other => other.to_string(),
        })?;
    let text = response
        .into_string()
        .map_err(|error| format!("read error: {error}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("invalid response JSON: {error}"))?;
    parse_chat_response(chat.provider, &json).map_err(|error| error.to_string())
}

fn chat_config(ai_config: &AiConfig) -> ChatConfig {
    let provider = match ai_config.provider {
        AiProvider::OpenAI => Provider::OpenAiCompatible,
        AiProvider::Anthropic => Provider::Anthropic,
        AiProvider::Ollama => Provider::Ollama,
    };
    // rsh's base_url contract matches the provider defaults (no trailing
    // path); jagent's endpoint() appends the per-provider path.
    ChatConfig {
        provider,
        api_key: ai_config.api_key.clone(),
        model: ai_config.model.clone(),
        base_url: ai_config.base_url.clone(),
        max_tokens: AGENT_MAX_TOKENS,
    }
}

fn environment_meta(share_context: bool) -> EnvironmentMeta {
    EnvironmentMeta {
        cwd: std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        shell: "rsh".to_string(),
        os: std::env::consts::OS.to_string(),
        git: if share_context { git_meta() } else { None },
    }
}

fn git_meta() -> Option<GitMeta> {
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())?;
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.is_empty());
    Some(GitMeta {
        branch,
        dirty,
        ahead: 0,
        behind: 0,
    })
}

/// Run one approved command through the rsh parser/executor in a forked child,
/// teeing combined stdout+stderr to the terminal while capturing a bounded
/// copy for the observation. Interactive/TTY-dependent programs will see a
/// pipe; the agent protocol already biases toward non-interactive commands.
fn run_captured(command: &str, state: &mut ShellState) -> (i32, String) {
    use nix::sys::wait::{waitpid, WaitStatus};
    use nix::unistd::{close, fork, pipe, read, ForkResult};
    use std::os::unix::io::{BorrowedFd, IntoRawFd};

    println!("  {}", dim(&format!("$ {command}")));
    let (r, w) = match pipe() {
        Ok(fds) => (fds.0.into_raw_fd(), fds.1.into_raw_fd()),
        Err(error) => return (1, format!("[rsh: pipe failed: {error}]")),
    };

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            close(r).ok();
            unsafe {
                nix::libc::dup2(w, 1);
                nix::libc::dup2(w, 2);
            }
            close(w).ok();
            state.interactive = false;
            match crate::parser::parse(command) {
                Ok(commands) => {
                    let mut code = 0;
                    for parsed in &commands {
                        code = crate::executor::execute_complete_command(parsed, state);
                    }
                    std::process::exit(code);
                }
                Err(error) => {
                    eprintln!("rsh: parse error: {error}");
                    std::process::exit(2);
                }
            }
        }
        Ok(ForkResult::Parent { child }) => {
            close(w).ok();
            let mut captured: Vec<u8> = Vec::new();
            let mut truncated = false;
            let mut buffer = [0_u8; 4096];
            let stdout = std::io::stdout();
            loop {
                match unsafe { read(BorrowedFd::borrow_raw(r), &mut buffer) } {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let chunk = &buffer[..count];
                        let mut out = stdout.lock();
                        let _ = out.write_all(chunk);
                        let _ = out.flush();
                        if captured.len() < MAX_CAPTURED_OUTPUT_BYTES {
                            let room = MAX_CAPTURED_OUTPUT_BYTES - captured.len();
                            captured.extend_from_slice(&chunk[..count.min(room)]);
                            if room <= count {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                }
            }
            close(r).ok();
            let exit_code = match waitpid(child, None) {
                Ok(WaitStatus::Exited(_, code)) => code,
                Ok(WaitStatus::Signaled(_, signal, _)) => 128 + signal as i32,
                _ => 1,
            };
            let mut output = String::from_utf8_lossy(&captured).to_string();
            if truncated {
                output.push_str("\n[rsh: further output not captured]");
            }
            (exit_code, output)
        }
        Err(error) => {
            close(r).ok();
            close(w).ok();
            (1, format!("[rsh: fork failed: {error}]"))
        }
    }
}

fn max_turns() -> u32 {
    std::env::var("RSH_AGENT_MAX_TURNS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|turns| *turns > 0)
        .unwrap_or(DEFAULT_MAX_TURNS)
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
    }
}

fn confirm(prompt: &str) -> bool {
    matches!(
        read_line(prompt).as_deref().map(str::trim),
        Some("y" | "yes")
    )
}

fn status_line(text: &str) {
    println!("{}", dim(&format!("[agent] {text}")));
}

fn styled(code: &str, text: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn emphasize(text: &str) -> String {
    styled("1", text)
}

fn warn(text: &str) -> String {
    styled("31", text)
}

fn dim(text: &str) -> String {
    styled("2", text)
}
