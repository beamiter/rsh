/// Hook system: precmd, preexec, chpwd hooks.

use crate::environment::ShellState;

pub fn run_hooks(hook_list: &[String], state: &mut ShellState) {
    for hook in hook_list {
        if let Some(body) = state.functions.get(hook).cloned() {
            crate::executor::execute_compound(&body, state);
        } else if let Ok(cmds) = crate::parser::parse(hook) {
            for cmd in &cmds {
                crate::executor::execute_complete_command(cmd, state);
            }
        }
    }
}
