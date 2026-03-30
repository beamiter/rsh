/// Job control: process groups, fg/bg, job table, async notifications.

use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{Pid, tcsetpgrp};
use std::fmt;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Running,
    Stopped,
    Done(i32),
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobStatus::Running => write!(f, "Running"),
            JobStatus::Stopped => write!(f, "Stopped"),
            JobStatus::Done(code) => write!(f, "Done({})", code),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: usize,
    pub pid: Pid,
    pub command: String,
    pub status: JobStatus,
    pub start_time: Instant,
}

pub struct JobTable {
    pub jobs: Vec<Job>,
    next_id: usize,
}

impl JobTable {
    pub fn new() -> Self {
        JobTable { jobs: Vec::new(), next_id: 1 }
    }

    pub fn add(&mut self, pid: Pid, command: String) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            pid,
            command,
            status: JobStatus::Running,
            start_time: Instant::now(),
        });
        id
    }

    pub fn get_by_id(&mut self, id: usize) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn get_last_stopped(&mut self) -> Option<&mut Job> {
        self.jobs.iter_mut().rev().find(|j| j.status == JobStatus::Stopped)
    }

    pub fn get_last(&mut self) -> Option<&mut Job> {
        self.jobs.iter_mut().rev()
            .find(|j| j.status == JobStatus::Running || j.status == JobStatus::Stopped)
    }

    pub fn remove_done(&mut self) {
        self.jobs.retain(|j| !matches!(j.status, JobStatus::Done(_)));
    }

    pub fn notify_done(&mut self) {
        self.notify_done_with_notification(Duration::from_secs(u64::MAX));
    }

    pub fn notify_done_with_notification(&mut self, threshold: Duration) {
        for job in &self.jobs {
            if let JobStatus::Done(code) = job.status {
                let elapsed = job.start_time.elapsed();
                eprintln!("[{}]+  Done({})  ({:.1}s)  {}", job.id, code, elapsed.as_secs_f64(), job.command);
                if elapsed > threshold {
                    send_notification(&job.command, code, elapsed);
                }
            }
        }
        self.remove_done();
    }

    pub fn check_background(&mut self) {
        for job in &mut self.jobs {
            if job.status == JobStatus::Running {
                match waitpid(job.pid, Some(WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED)) {
                    Ok(WaitStatus::Exited(_, code)) => {
                        job.status = JobStatus::Done(code);
                    }
                    Ok(WaitStatus::Signaled(_, _, _)) => {
                        job.status = JobStatus::Done(128);
                    }
                    Ok(WaitStatus::Stopped(_, _)) => {
                        job.status = JobStatus::Stopped;
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn print_jobs(&self) {
        for job in &self.jobs {
            let elapsed = job.start_time.elapsed();
            println!("[{}]+  {}  ({:.1}s)  {}", job.id, job.status, elapsed.as_secs_f64(), job.command);
        }
    }

    pub fn wait_fg(&mut self, pid: Pid) -> i32 {
        loop {
            match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
                Ok(WaitStatus::Exited(_, code)) => return code,
                Ok(WaitStatus::Signaled(_, sig, _)) => return 128 + sig as i32,
                Ok(WaitStatus::Stopped(_, _)) => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.pid == pid) {
                        job.status = JobStatus::Stopped;
                        eprintln!("\n[{}]+  Stopped                    {}", job.id, job.command);
                    }
                    return 148;
                }
                Err(_) => return 1,
                _ => continue,
            }
        }
    }

    pub fn continue_fg(&mut self, id: usize) -> i32 {
        if let Some(job) = self.get_by_id(id) {
            let pid = job.pid;
            job.status = JobStatus::Running;
            eprintln!("{}", job.command);
            let shell_pgid = nix::unistd::getpgrp();
            tcsetpgrp(std::io::stdin(), pid).ok();
            kill(pid, Signal::SIGCONT).ok();
            let code = self.wait_fg(pid);
            tcsetpgrp(std::io::stdin(), shell_pgid).ok();
            code
        } else {
            eprintln!("rsh: fg: {}: no such job", id);
            1
        }
    }

    pub fn continue_bg(&mut self, id: usize) -> i32 {
        if let Some(job) = self.get_by_id(id) {
            job.status = JobStatus::Running;
            eprintln!("[{}]+ {} &", job.id, job.command);
            kill(job.pid, Signal::SIGCONT).ok();
            0
        } else {
            eprintln!("rsh: bg: {}: no such job", id);
            1
        }
    }
}

fn send_notification(command: &str, exit_code: i32, elapsed: Duration) {
    let status = if exit_code == 0 { "completed" } else { "failed" };
    let summary = format!("Command {}", status);
    let body = format!("{} ({:.1}s)", command, elapsed.as_secs_f64());

    // OSC 777 terminal notification (iTerm2, Kitty, etc.)
    eprint!("\x1b]777;notify;{};{}\x07", summary, body);

    // Also try notify-send (Linux desktop)
    std::process::Command::new("notify-send")
        .args([&summary, &body])
        .spawn()
        .ok();
}
