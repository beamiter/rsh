/// Signal handling for the shell.

use nix::libc;
use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use std::sync::atomic::{AtomicBool, Ordering};

pub static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);
pub static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);
pub static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigchld_handler(_: libc::c_int) {
    SIGCHLD_RECEIVED.store(true, Ordering::SeqCst);
}

extern "C" fn sigwinch_handler(_: libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::SeqCst);
}

extern "C" fn sigint_handler(_: libc::c_int) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
}

pub fn install_shell_signals() {
    unsafe {
        let sa_ignore = SigAction::new(SigHandler::SigIgn, SaFlags::SA_RESTART, SigSet::empty());

        // Shell ignores these (children will get default)
        signal::sigaction(Signal::SIGTSTP, &sa_ignore).ok();
        signal::sigaction(Signal::SIGTTIN, &sa_ignore).ok();
        signal::sigaction(Signal::SIGTTOU, &sa_ignore).ok();
        signal::sigaction(Signal::SIGPIPE, &sa_ignore).ok();

        // Custom handlers
        let sa_chld = SigAction::new(
            SigHandler::Handler(sigchld_handler),
            SaFlags::SA_RESTART | SaFlags::SA_NOCLDSTOP,
            SigSet::empty(),
        );
        signal::sigaction(Signal::SIGCHLD, &sa_chld).ok();

        let sa_winch = SigAction::new(
            SigHandler::Handler(sigwinch_handler),
            SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        signal::sigaction(Signal::SIGWINCH, &sa_winch).ok();

        let sa_int = SigAction::new(
            SigHandler::Handler(sigint_handler),
            SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        signal::sigaction(Signal::SIGINT, &sa_int).ok();
    }
}

/// Reset signal handlers to default (called in child after fork).
pub fn reset_child_signals() {
    unsafe {
        let sa_default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
        signal::sigaction(Signal::SIGINT, &sa_default).ok();
        signal::sigaction(Signal::SIGTSTP, &sa_default).ok();
        signal::sigaction(Signal::SIGTTIN, &sa_default).ok();
        signal::sigaction(Signal::SIGTTOU, &sa_default).ok();
        signal::sigaction(Signal::SIGPIPE, &sa_default).ok();
        signal::sigaction(Signal::SIGCHLD, &sa_default).ok();
    }
}
