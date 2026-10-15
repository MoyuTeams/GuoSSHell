#[cfg(unix)]
use super::{Failure, Shared, TreeOutcome};
#[cfg(unix)]
use portable_pty::{Child, CommandBuilder, SlavePty};

#[cfg(windows)]
#[path = "platform/windows.rs"]
mod windows;
#[cfg(windows)]
pub(super) use windows::{
    Identity, ThreadTarget, Tree, cancel_write, hold_cwd, poll_exit, prepare_writer,
    release_writer_target, validate_environment, wait_child,
};

#[cfg(unix)]
pub(super) struct Tree {
    spawned: bool,
}
#[cfg(unix)]
impl Tree {
    pub fn new() -> Result<Self, Failure> {
        Ok(Self { spawned: false })
    }
    pub fn spawn(
        &mut self,
        slave: &dyn SlavePty,
        command: CommandBuilder,
    ) -> Result<Box<dyn Child + Send + Sync>, Failure> {
        let child = slave.spawn_command(command).map_err(|error| Failure {
            operation: "spawn",
            source: std::sync::Arc::from(error.into_boxed_dyn_error()),
        })?;
        self.spawned = true;
        Ok(child)
    }
    pub fn terminate(
        &mut self,
        child: Option<&mut (dyn Child + Send + Sync + 'static)>,
    ) -> Result<(), Failure> {
        // 仅 child 自有 killer；不对未知 PID/进程组发信号，不据此保证后代已清。
        if let Some(child) = child {
            child
                .kill()
                .map_err(|error| Failure::io("child-kill", error))?;
        }
        Ok(())
    }
    pub fn poll_finished(&self) -> Result<Option<TreeOutcome>, Failure> {
        Ok(Some(if self.spawned {
            TreeOutcome::UnverifiedUnix
        } else {
            TreeOutcome::NotSpawned
        }))
    }
}
#[cfg(unix)]
pub(super) fn poll_exit(child: &mut dyn Child) -> Result<Option<u32>, Failure> {
    child
        .try_wait()
        .map(|status| status.map(|status| status.exit_code()))
        .map_err(|error| Failure::io("child-poll", error))
}
#[cfg(unix)]
pub(super) fn wait_child(child: &mut dyn Child) -> Result<portable_pty::ExitStatus, Failure> {
    child
        .wait()
        .map_err(|error| Failure::io("child-wait", error))
}
#[cfg(unix)]
pub(super) fn prepare_writer(_: &Shared) -> Result<(), Failure> {
    Ok(())
}
#[cfg(unix)]
pub(super) fn cancel_write(_: &Shared) -> Result<(), Failure> {
    Ok(())
}
#[cfg(unix)]
pub(super) fn release_writer_target(_: &Shared) {}
