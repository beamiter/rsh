/// Job control: process groups, fg/bg, job table.

use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::fmt;

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
        for job in &self.jobs {
            if let JobStatus::Done(code) = job.status {
                eprintln!("[{}]+  Done({})                    {}", job.id, code, job.command);
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
                    _ => {} // still running
                }
            }
        }
    }

    pub fn print_jobs(&self) {
        for job in &self.jobs {
            println!("[{}]+  {}                    {}", job.id, job.status, job.command);
        }
    }

    pub fn wait_fg(&mut self, pid: Pid) -> i32 {
        loop {
            match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
                Ok(WaitStatus::Exited(_, code)) => return code,
                Ok(WaitStatus::Signaled(_, sig, _)) => return 128 + sig as i32,
                Ok(WaitStatus::Stopped(_, _)) => {
                    // Job got stopped (Ctrl-Z)
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.pid == pid) {
                        job.status = JobStatus::Stopped;
                        eprintln!("\n[{}]+  Stopped                    {}", job.id, job.command);
                    }
                    return 148; // 128 + SIGTSTP
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
            kill(pid, Signal::SIGCONT).ok();
            self.wait_fg(pid)
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
