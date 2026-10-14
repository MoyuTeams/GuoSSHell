//! 应用自有长期 Local owner；所有阻塞 PTY 操作都由普通 owned worker 执行。

#[path = "production/platform.rs"]
mod platform;

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{Notify, oneshot};

pub(crate) const IO_CHUNK: usize = 8192;
const ITEMS: usize = 8;
const BYTES: usize = IO_CHUNK * ITEMS;
const SOFT_CLOSE: Duration = Duration::from_millis(250);

pub(crate) struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// None 继承应用当前目录；Some 必须是存在的目录，不回退到用户目录。
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<OsString, OsString>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Geometry {
    fn checked(self) -> Result<PtySize, Failure> {
        if self.cols == 0
            || self.rows == 0
            || (cfg!(windows) && (self.cols > i16::MAX as u16 || self.rows > i16::MAX as u16))
        {
            return Err(Failure::invalid("geometry", "终端尺寸超出平台范围"));
        }
        Ok(PtySize {
            cols: self.cols,
            rows: self.rows,
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub operation: &'static str,
    pub source: Arc<dyn std::error::Error + Send + Sync>,
}

impl Failure {
    fn io(operation: &'static str, error: io::Error) -> Self {
        Self {
            operation,
            source: Arc::new(error),
        }
    }

    fn invalid(operation: &'static str, message: &'static str) -> Self {
        Self::io(
            operation,
            io::Error::new(io::ErrorKind::InvalidInput, message),
        )
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.operation, self.source)
    }
}
impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TreeOutcome {
    #[cfg(windows)]
    ContainedWindows,
    /// generic PTY 的 child 已 reap；Unix session/process-group 清理仍须父接缝。
    #[cfg(unix)]
    UnverifiedUnix,
    NotSpawned,
}

#[derive(Clone, Debug)]
pub(crate) struct Completion {
    pub exit_code: Option<u32>,
    pub cancelled: bool,
    pub tree: TreeOutcome,
    pub primary: Option<Failure>,
    pub cleanup_errors: Vec<Failure>,
}

#[derive(Clone, Debug)]
pub(crate) struct ClosingStatus {
    pub pending: bool,
    pub primary: Option<Failure>,
    pub last_cleanup_error: Option<Failure>,
}

#[derive(Debug)]
pub(crate) enum Event {
    Output(Vec<u8>),
    Closed(Completion),
}

#[derive(Debug)]
/// committed 只统计成功返回的写；失败的在途调用可能另有副作用，不能作为重放偏移。
pub(crate) enum WriteOutcome {
    Complete(usize),
    Closing {
        committed: usize,
        error: Option<Failure>,
    },
    Failed {
        committed: usize,
        error: Failure,
    },
}

pub(crate) struct WriteTicket(oneshot::Receiver<WriteOutcome>);
impl WriteTicket {
    pub(crate) async fn finish(self) -> Result<WriteOutcome, Failure> {
        self.0
            .await
            .map_err(|_| Failure::invalid("write-worker", "写线程未交付结果"))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    Closing,
    Capacity,
}

struct Input {
    bytes: Vec<u8>,
    reply: oneshot::Sender<WriteOutcome>,
}

struct Resize {
    size: PtySize,
    reply: oneshot::Sender<Result<(), Failure>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observation {
    pub reader_started: bool,
    pub writer_started: bool,
    pub master_opened: bool,
    pub reader_bound: bool,
    pub writer_bound: bool,
    pub terminate_attempts: usize,
    pub terminate_failures: usize,
    pub input_bytes: usize,
    pub input_items: usize,
    pub output_bytes: usize,
    pub output_items: usize,
    pub write_entered: usize,
    pub write_returned: usize,
    pub writer_dropped: bool,
    pub writer_joined: bool,
    pub master_dropped: bool,
    pub reader_dropped: bool,
    pub reader_joined: bool,
    pub child_reaped: bool,
    /// PTY 图与 reader/writer 已收口；控制线程 join 权仍由 LocalOwner 保持到 Drop。
    pub cleanup_finished: bool,
    pub controller_joined: bool,
    pub writer_joined_at: usize,
    pub child_reaped_at: usize,
    pub tree_dropped_at: usize,
    pub master_dropped_at: usize,
    pub reader_joined_at: usize,
}

struct State {
    cancel: bool,
    stop_write: bool,
    input: VecDeque<Input>,
    retired_input: VecDeque<Vec<u8>>,
    output: VecDeque<Vec<u8>>,
    resize: VecDeque<Resize>,
    ready: Option<Result<(), Failure>>,
    completion: Option<Completion>,
    observation: Observation,
    sequence: usize,
    primary: Option<Failure>,
    closing: bool,
    last_cleanup_error: Option<Failure>,
    #[cfg(windows)]
    identity: Option<Arc<platform::Identity>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    notify: Notify,
    #[cfg(windows)]
    writer_target: Mutex<Option<platform::ThreadTarget>>,
    #[cfg(test)]
    hooks: Hooks,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    fn signal(&self) {
        self.changed.notify_all();
        self.notify.notify_waiters();
    }

    fn cancel(&self) {
        let mut state = lock(&self.state);
        state.cancel = true;
        state.stop_write = true;
        drop(state);
        self.signal();
    }

    fn failure(&self, error: Failure) {
        lock(&self.state).primary.get_or_insert(error);
        self.cancel();
    }
}

/// start 返回即接管不可取消的 spawn；ready Future 被放弃不会放弃 child。
/// Drop 是同步的最后责任根，必须放在父关闭执行面而非 UI 回调中。
pub(crate) struct LocalOwner {
    shared: Arc<Shared>,
    controller: Option<JoinHandle<()>>,
    closed_delivered: bool,
}

impl LocalOwner {
    pub(crate) fn start(
        launch: Launch,
        term: OsString,
        geometry: Geometry,
    ) -> Result<Self, Failure> {
        Self::start_inner(
            launch,
            term,
            geometry,
            #[cfg(test)]
            Hooks::default(),
        )
    }

    fn start_inner(
        launch: Launch,
        term: OsString,
        geometry: Geometry,
        #[cfg(test)] hooks: Hooks,
    ) -> Result<Self, Failure> {
        let size = geometry.checked()?;
        if launch.program.is_empty() || term.is_empty() {
            return Err(Failure::invalid("launch", "必须显式提供 program 与 TERM"));
        }
        if launch.program.as_encoded_bytes().contains(&0)
            || term.as_encoded_bytes().contains(&0)
            || launch
                .args
                .iter()
                .any(|arg| arg.as_encoded_bytes().contains(&0))
            || launch.env.iter().any(|(key, value)| {
                key.is_empty()
                    || key
                        .as_encoded_bytes()
                        .iter()
                        .any(|byte| *byte == 0 || *byte == b'=')
                    || value.as_encoded_bytes().contains(&0)
            })
        {
            return Err(Failure::invalid(
                "launch",
                "启动参数包含无效的 NUL 或环境变量名",
            ));
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                cancel: false,
                stop_write: false,
                input: VecDeque::with_capacity(ITEMS),
                output: VecDeque::with_capacity(ITEMS),
                retired_input: VecDeque::with_capacity(ITEMS),
                resize: VecDeque::with_capacity(ITEMS),
                ready: None,
                completion: None,
                observation: Observation::default(),
                sequence: 0,
                primary: None,
                closing: false,
                last_cleanup_error: None,
                #[cfg(windows)]
                identity: None,
            }),
            changed: Condvar::new(),
            notify: Notify::new(),
            #[cfg(windows)]
            writer_target: Mutex::new(None),
            #[cfg(test)]
            hooks,
        });
        let worker_shared = Arc::clone(&shared);
        let controller = thread::Builder::new()
            .name("local-wait-owner".into())
            .spawn(move || control(worker_shared, launch, term, size))
            .map_err(|error| Failure::io("wait-worker", error))?;
        Ok(Self {
            shared,
            controller: Some(controller),
            closed_delivered: false,
        })
    }

    pub(crate) async fn ready(&self) -> Result<(), Failure> {
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = lock(&self.shared.state).ready.clone() {
                return result;
            }
            notified.await;
        }
    }

    /// 不等待 OS 写；满额时调用者保留原输入并等待后重试，不复制超限输入。
    pub(crate) fn try_write(&self, bytes: &[u8]) -> Result<WriteTicket, AdmissionError> {
        let mut state = lock(&self.shared.state);
        if state.stop_write {
            return Err(AdmissionError::Closing);
        }
        if bytes.len() > IO_CHUNK
            || state.observation.input_items == ITEMS
            || state.observation.input_bytes + bytes.len() > BYTES
        {
            return Err(AdmissionError::Capacity);
        }
        let (reply, receive) = oneshot::channel();
        state.observation.input_bytes += bytes.len();
        state.observation.input_items += 1;
        state.input.push_back(Input {
            bytes: bytes.to_vec(),
            reply,
        });
        drop(state);
        self.shared.signal();
        Ok(WriteTicket(receive))
    }

    pub(crate) async fn resize(&self, geometry: Geometry) -> Result<(), Failure> {
        let size = geometry.checked()?;
        let (reply, receive) = oneshot::channel();
        {
            let mut state = lock(&self.shared.state);
            if state.stop_write {
                return Err(Failure::invalid("resize", "owner 正在关闭"));
            }
            if state.resize.len() == ITEMS {
                return Err(Failure::invalid("resize", "控制队列已满"));
            }
            state.resize.push_back(Resize { size, reply });
        }
        self.shared.signal();
        receive
            .await
            .map_err(|_| Failure::invalid("resize-worker", "控制线程未交付结果"))?
    }

    pub(crate) fn cancel(&self) {
        self.shared.cancel();
    }

    pub(crate) fn observation(&self) -> Observation {
        lock(&self.shared.state).observation
    }

    pub(crate) fn closing_status(&self) -> ClosingStatus {
        let state = lock(&self.shared.state);
        ClosingStatus {
            pending: state.closing && state.completion.is_none(),
            primary: state.primary.clone(),
            last_cleanup_error: state.last_cleanup_error.clone(),
        }
    }

    #[cfg(windows)]
    pub(crate) fn identity(&self) -> Option<Arc<platform::Identity>> {
        lock(&self.shared.state).identity.clone()
    }

    /// 所有尾输出先于 Closed；此接口不执行系统 read/write/wait 或 join。
    pub(crate) async fn next_event(&mut self) -> Option<Event> {
        if self.closed_delivered {
            return None;
        }
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = lock(&self.shared.state);
                if let Some(bytes) = state.output.pop_front() {
                    state.observation.output_bytes -= bytes.len();
                    state.observation.output_items -= 1;
                    self.shared.changed.notify_all();
                    return Some(Event::Output(bytes));
                }
                if let Some(completion) = state.completion.clone() {
                    self.closed_delivered = true;
                    return Some(Event::Closed(completion));
                }
            }
            notified.await;
        }
    }

    /// 超时由父包装 Future；放弃等待仍保留本 owner 与所有 join 权。
    pub(crate) async fn close(&self) -> Completion {
        self.cancel();
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = lock(&self.shared.state).completion.clone() {
                return result;
            }
            notified.await;
        }
    }
}

impl Drop for LocalOwner {
    fn drop(&mut self) {
        self.shared.cancel();
        if let Some(controller) = self.controller.take() {
            // 不 detach，不把超时作为回收；阻塞兜底保留责任直到控制线程真正返回。
            if let Err(payload) = controller.join() {
                lock(&self.shared.state)
                    .primary
                    .get_or_insert_with(|| panic_failure("controller-join", payload));
            }
            lock(&self.shared.state).observation.controller_joined = true;
        }
    }
}

type Reader = Box<dyn Read + Send>;
type Writer = Box<dyn Write + Send>;

struct Resources {
    shared: Arc<Shared>,
    reader_send: Option<mpsc::Sender<Reader>>,
    writer_send: Option<mpsc::Sender<Writer>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    tree: Option<platform::Tree>,
    active_resize: Option<Resize>,
    cleaned: bool,
}

impl Resources {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            reader_send: None,
            writer_send: None,
            reader: None,
            writer: None,
            master: None,
            child: None,
            tree: None,
            active_resize: None,
            cleaned: false,
        }
    }

    fn prepare_workers(&mut self) -> Result<(), Failure> {
        #[cfg(test)]
        if self.shared.hooks.fail_before_workers {
            return Err(Failure::io(
                "reader-worker",
                io::Error::from_raw_os_error(8),
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        self.reader = Some(
            thread::Builder::new()
                .name("local-reader".into())
                .spawn(move || read_worker(shared, receiver))
                .map_err(|error| Failure::io("reader-worker", error))?,
        );
        self.reader_send = Some(sender);
        lock(&self.shared.state).observation.reader_started = true;
        #[cfg(test)]
        if self.shared.hooks.fail_after_reader_worker {
            return Err(Failure::io(
                "writer-worker",
                io::Error::from_raw_os_error(8),
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let (ready_send, ready_receive) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        self.writer = Some(
            thread::Builder::new()
                .name("local-writer".into())
                .spawn(move || write_worker(shared, receiver, ready_send))
                .map_err(|error| Failure::io("writer-worker", error))?,
        );
        self.writer_send = Some(sender);
        lock(&self.shared.state).observation.writer_started = true;
        ready_receive
            .recv()
            .map_err(|_| Failure::invalid("writer-worker", "写线程未就绪"))?
    }

    fn initialize(&mut self, launch: Launch, term: OsString, size: PtySize) -> Result<(), Failure> {
        #[cfg(test)]
        if self.shared.hooks.fail_tree {
            return Err(Failure::io("job", io::Error::from_raw_os_error(5)));
        }
        self.tree = Some(platform::Tree::new()?);
        #[cfg(all(test, windows))]
        if let Some(tree) = self.tree.as_mut() {
            tree.set_faults(self.shared.hooks.terminate_failures.clone());
        }
        self.prepare_workers()?;
        let mut command = CommandBuilder::new(&launch.program);
        command.args(&launch.args);
        let cwd = match launch.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir().map_err(|error| Failure::io("cwd", error))?,
        };
        // 上游 Windows 对错误目录可能回退；验证绝对目录并跨 spawn 保持目录句柄。
        let cwd = std::fs::canonicalize(cwd).map_err(|error| Failure::io("cwd", error))?;
        if !cwd.is_dir() {
            return Err(Failure::invalid("cwd", "cwd 不是目录"));
        }
        #[cfg(windows)]
        let _cwd_guard = platform::hold_cwd(&cwd)?;
        command.cwd(cwd);
        for (key, value) in launch.env {
            command.env(key, value);
        }
        command.env("TERM", term);
        #[cfg(windows)]
        platform::validate_environment(&command)?;
        let pair = portable_pty::native_pty_system()
            .openpty(size)
            .map_err(|error| Failure {
                operation: "openpty",
                source: Arc::from(error.into_boxed_dyn_error()),
            })?;
        self.master = Some(pair.master);
        lock(&self.shared.state).observation.master_opened = true;
        if lock(&self.shared.state).cancel {
            drop(pair.slave);
            return Err(cancelled());
        }
        #[cfg(test)]
        self.shared.hooks.before_spawn();
        let child = self
            .tree
            .as_mut()
            .ok_or_else(cancelled)?
            .spawn(&*pair.slave, command);
        drop(pair.slave);
        self.child = Some(child?);
        #[cfg(windows)]
        {
            let identity =
                platform::Identity::capture(&**self.child.as_ref().ok_or_else(cancelled)?)?;
            self.tree
                .as_ref()
                .ok_or_else(cancelled)?
                .verify(&identity)?;
            lock(&self.shared.state).identity = Some(Arc::new(identity));
        }
        // spawn fallback 可以替换端点；稳定 worker 已预留，但端点必须在返回后取得。
        let master = self.master.as_ref().ok_or_else(cancelled)?;
        let reader = master.try_clone_reader().map_err(|error| Failure {
            operation: "reader-endpoint",
            source: Arc::from(error.into_boxed_dyn_error()),
        })?;
        self.reader_send
            .take()
            .ok_or_else(cancelled)?
            .send(reader)
            .map_err(|_| Failure::invalid("reader-worker", "读线程已结束"))?;
        lock(&self.shared.state).observation.reader_bound = true;
        #[cfg(all(test, windows))]
        if let Some(program) = &self.shared.hooks.library_panic_after_reader {
            // 真实上游参数解析故障；原 owner 的 child、端点与 worker 仍在 Resources 内。
            let probe = portable_pty::native_pty_system()
                .openpty(size)
                .map_err(|error| Failure {
                    operation: "panic-probe-pty",
                    source: Arc::from(error.into_boxed_dyn_error()),
                })?;
            let mut command = CommandBuilder::new(program);
            command.env("PATH", std::env::temp_dir());
            command.env("PATHEXT", "é");
            let _owned = self
                .tree
                .as_mut()
                .ok_or_else(cancelled)?
                .spawn(&*probe.slave, command)?;
            return Err(Failure::invalid("panic-probe", "预期参数解析异常"));
        }
        #[cfg(test)]
        if self.shared.hooks.fail_after_reader {
            return Err(Failure::invalid("partial-init", "端点交接失败"));
        }
        let writer = master.take_writer().map_err(|error| Failure {
            operation: "writer-endpoint",
            source: Arc::from(error.into_boxed_dyn_error()),
        })?;
        self.writer_send
            .take()
            .ok_or_else(cancelled)?
            .send(writer)
            .map_err(|_| Failure::invalid("writer-worker", "写线程已结束"))?;
        lock(&self.shared.state).observation.writer_bound = true;
        if lock(&self.shared.state).cancel {
            return Err(cancelled());
        }
        Ok(())
    }

    fn cleanup(&mut self, natural: Option<u32>) -> Completion {
        {
            let mut state = lock(&self.shared.state);
            state.stop_write = true;
            state.closing = true;
        }
        self.shared.signal();
        drop(self.writer_send.take());
        let mut errors = Vec::new();
        let start = Instant::now();
        let mut exit_code = natural;
        let mut next_terminate = start;
        let mut retry_delay = Duration::from_millis(10);
        let tree = loop {
            let writer_done = self.writer.as_ref().is_none_or(JoinHandle::is_finished);
            if !writer_done {
                if let Err(error) = platform::cancel_write(&self.shared) {
                    self.cleanup_error(error, &mut errors);
                }
            } else {
                if let Some(worker) = self.writer.take() {
                    if let Err(payload) = worker.join() {
                        self.cleanup_error(panic_failure("writer-join", payload), &mut errors);
                    }
                    let mut state = lock(&self.shared.state);
                    state.observation.writer_joined = true;
                    state.sequence += 1;
                    state.observation.writer_joined_at = state.sequence;
                }
                platform::release_writer_target(&self.shared);
                // 无 writer 与已实际 join 的 writer 使用同一结算边界。
                self.settle_pending();
            }
            let mut child_signaled = self.child.is_none();
            if let Some(child) = self.child.as_mut() {
                match platform::poll_exit(&mut **child) {
                    Ok(Some(code)) => {
                        exit_code = Some(code);
                        child_signaled = true;
                    }
                    Ok(None) => {}
                    Err(error) => self.cleanup_error(error, &mut errors),
                }
            }
            let tree_done = match self.tree.as_mut() {
                Some(tree) => tree.poll_finished(),
                None => Ok(Some(TreeOutcome::NotSpawned)),
            };
            let outcome = match tree_done {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.cleanup_error(error, &mut errors);
                    None
                }
            };
            if writer_done && child_signaled {
                let reap = self
                    .child
                    .as_mut()
                    .map(|child| platform::wait_child(&mut **child));
                match reap {
                    Some(Ok(status)) => {
                        exit_code = Some(status.exit_code());
                        drop(self.child.take());
                        let mut state = lock(&self.shared.state);
                        state.observation.child_reaped = true;
                        state.sequence += 1;
                        state.observation.child_reaped_at = state.sequence;
                    }
                    Some(Err(error)) => self.cleanup_error(error, &mut errors),
                    None => {}
                }
                if self.child.is_none()
                    && let Some(outcome) = outcome
                {
                    break outcome;
                }
            }
            // child 未 signaled 或 Job 仍有后代时，在同一状态机持续有限退避重试。
            // 只有写线程已结束或 softclose 到期才进入 hard cleanup；失败不消耗重试权。
            if (writer_done || start.elapsed() >= SOFT_CLOSE)
                && (!child_signaled || outcome.is_none())
                && Instant::now() >= next_terminate
            {
                if let Some(tree) = self.tree.as_mut() {
                    lock(&self.shared.state).observation.terminate_attempts += 1;
                    if let Err(error) = tree.terminate(if child_signaled {
                        None
                    } else {
                        self.child.as_deref_mut()
                    }) {
                        lock(&self.shared.state).observation.terminate_failures += 1;
                        self.cleanup_error(error, &mut errors);
                    }
                }
                next_terminate = Instant::now() + retry_delay;
                retry_delay = (retry_delay * 2).min(Duration::from_millis(80));
            }
            thread::sleep(Duration::from_millis(2));
        };
        if let Some(tree) = self.tree.take() {
            drop(tree);
            let mut state = lock(&self.shared.state);
            state.sequence += 1;
            state.observation.tree_dropped_at = state.sequence;
        }
        // 端点复制失败仍由同一预留 reader 承接关闭 drain，不能直接丢弃 HPCON。
        // 持久复制失败保留责任根，不将超时记作清理完成。
        if self.reader_send.is_some() && self.master.is_some() {
            while let Some(master) = self.master.as_ref() {
                match master.try_clone_reader() {
                    Ok(reader) => {
                        if let Some(sender) = self.reader_send.take() {
                            match sender.send(reader) {
                                Ok(()) => lock(&self.shared.state).observation.reader_bound = true,
                                Err(_) => self.cleanup_error(
                                    Failure::io(
                                        "reader-worker",
                                        io::Error::other("读线程未接收端点"),
                                    ),
                                    &mut errors,
                                ),
                            }
                        }
                        break;
                    }
                    Err(error) => {
                        self.cleanup_error(
                            Failure {
                                operation: "reader-endpoint",
                                source: Arc::from(error.into_boxed_dyn_error()),
                            },
                            &mut errors,
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
        drop(self.reader_send.take());
        // slave 已释放；只有这一 master 仍持有 ConPTY 的 strong 控制根。
        // reader 在关闭最后 HPCON 时持续 drain，不在取消状态等待 consumer。
        if let Some(master) = self.master.take() {
            drop(master);
            let mut state = lock(&self.shared.state);
            state.observation.master_dropped = true;
            state.sequence += 1;
            state.observation.master_dropped_at = state.sequence;
        }
        if let Some(worker) = self.reader.take() {
            if let Err(payload) = worker.join() {
                self.cleanup_error(panic_failure("reader-join", payload), &mut errors);
            }
            let mut state = lock(&self.shared.state);
            state.observation.reader_joined = true;
            state.sequence += 1;
            state.observation.reader_joined_at = state.sequence;
        }
        let mut state = lock(&self.shared.state);
        state.observation.cleanup_finished = true;
        self.cleaned = true;
        Completion {
            exit_code,
            cancelled: state.cancel,
            tree,
            primary: state.primary.clone(),
            cleanup_errors: errors,
        }
    }

    fn settle_pending(&mut self) {
        let mut state = lock(&self.shared.state);
        state.stop_write = true;
        let error = state.primary.clone();
        for input in state.input.drain(..) {
            let Input { bytes, reply } = input;
            drop(bytes);
            let _ = reply.send(WriteOutcome::Closing {
                committed: 0,
                error: error.clone(),
            });
        }
        state.retired_input.clear();
        state.observation.input_bytes = 0;
        state.observation.input_items = 0;
        for resize in state.resize.drain(..) {
            let _ = resize
                .reply
                .send(Err(error.clone().unwrap_or_else(cancelled)));
        }
        if let Some(resize) = self.active_resize.take() {
            let _ = resize.reply.send(Err(error.unwrap_or_else(cancelled)));
        }
    }

    fn cleanup_error(&self, error: Failure, errors: &mut Vec<Failure>) {
        let mut state = lock(&self.shared.state);
        state.primary.get_or_insert_with(|| error.clone());
        state.last_cleanup_error = Some(error.clone());
        drop(state);
        if errors.len() < ITEMS {
            errors.push(error);
        }
        self.shared.signal();
    }

    fn finish_cleanup(&mut self, natural: Option<u32>) -> Completion {
        let mut panic_errors = Vec::new();
        loop {
            match guarded("cleanup-panic", || Ok(self.cleanup(natural))) {
                Ok(mut completion) => {
                    for error in panic_errors {
                        if completion.cleanup_errors.len() < ITEMS {
                            completion.cleanup_errors.push(error);
                        }
                    }
                    return completion;
                }
                Err(error) => {
                    self.shared.cancel();
                    self.cleanup_error(error, &mut panic_errors);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        if !self.cleaned {
            self.shared.cancel();
            let completion = self.finish_cleanup(None);
            publish_completion(&self.shared, completion);
        }
    }
}

fn cancelled() -> Failure {
    Failure::io(
        "cancel",
        io::Error::new(io::ErrorKind::Interrupted, "Local 已取消"),
    )
}

fn control(shared: Arc<Shared>, launch: Launch, term: OsString, size: PtySize) {
    #[cfg(test)]
    if let Some(barrier) = &shared.hooks.before_initialize {
        barrier.wait();
        barrier.wait();
    }
    // 捕获边界之外的 Resources 始终保留所有权；异常不能展开并丢掉清理结果。
    let mut resources = Resources::new(Arc::clone(&shared));
    let result = guarded("controller-panic", || {
        resources.initialize(launch, term, size)?;
        lock(&shared.state).ready = Some(Ok(()));
        shared.signal();
        loop {
            let resize = {
                let mut state = lock(&shared.state);
                if state.cancel {
                    return Ok(None);
                }
                state.resize.pop_front()
            };
            if let Some(resize) = resize {
                let size = resize.size;
                resources.active_resize = Some(resize);
                let result = resources
                    .master
                    .as_ref()
                    .ok_or_else(cancelled)
                    .and_then(|master| {
                        master.resize(size).map_err(|error| Failure {
                            operation: "resize",
                            source: Arc::from(error.into_boxed_dyn_error()),
                        })
                    });
                if let Err(error) = &result {
                    shared.failure(error.clone());
                }
                if let Some(resize) = resources.active_resize.take() {
                    let _ = resize.reply.send(result);
                }
            }
            if let Some(child) = resources.child.as_mut() {
                match platform::poll_exit(&mut **child) {
                    Ok(Some(code)) => {
                        return Ok(Some(code));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Err(error);
                    }
                }
            }
            let state = lock(&shared.state);
            let _ = shared.changed.wait_timeout(state, Duration::from_millis(5));
        }
    });
    let natural = match result {
        Ok(code) => code,
        Err(error) => {
            shared.failure(error);
            None
        }
    };
    let completion = resources.finish_cleanup(natural);
    publish_completion(&shared, completion);
}

fn guarded<T>(
    operation: &'static str,
    action: impl FnOnce() -> Result<T, Failure>,
) -> Result<T, Failure> {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(result) => result,
        Err(payload) => Err(panic_failure(operation, payload)),
    }
}

fn panic_failure(operation: &'static str, payload: Box<dyn std::any::Any + Send>) -> Failure {
    let message = if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "控制操作返回非文本 panic".to_owned()
    };
    Failure::io(operation, io::Error::other(message))
}

fn publish_completion(shared: &Shared, completion: Completion) {
    let mut state = lock(&shared.state);
    state.stop_write = true;
    if state.ready.is_none() {
        state.ready = Some(Err(completion.primary.clone().unwrap_or_else(cancelled)));
    }
    state.completion = Some(completion);
    drop(state);
    shared.signal();
}

fn read_worker(shared: Arc<Shared>, receiver: mpsc::Receiver<Reader>) {
    if let Ok(mut reader) = receiver.recv() {
        let mut buffer = [0u8; IO_CHUNK];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let mut state = lock(&shared.state);
                    while !state.cancel
                        && (state.output.len() == ITEMS
                            || state.observation.output_bytes + count > BYTES)
                    {
                        state = shared
                            .changed
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    if !state.cancel {
                        state.output.push_back(buffer[..count].to_vec());
                        state.observation.output_bytes += count;
                        state.observation.output_items += 1;
                    }
                    drop(state);
                    shared.signal();
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // Unix PTY slave 关闭时 master read 的 EIO 是 EOF。
                #[cfg(unix)]
                Err(error) if error.raw_os_error() == Some(5) => break,
                Err(error) => {
                    if !lock(&shared.state).cancel {
                        shared.failure(Failure::io("read", error));
                    }
                    break;
                }
            }
        }
        drop(reader);
        lock(&shared.state).observation.reader_dropped = true;
    }
    shared.signal();
}

fn write_worker(
    shared: Arc<Shared>,
    receiver: mpsc::Receiver<Writer>,
    ready: mpsc::Sender<Result<(), Failure>>,
) {
    if let Err(error) = platform::prepare_writer(&shared) {
        let _ = ready.send(Err(error));
        return;
    }
    let _ = ready.send(Ok(()));
    if let Ok(mut writer) = receiver.recv() {
        loop {
            let input = {
                let mut state = lock(&shared.state);
                while !state.stop_write && state.input.is_empty() {
                    state = shared
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                if state.stop_write {
                    break;
                }
                state.input.pop_front()
            };
            let Some(input) = input else {
                continue;
            };
            let mut committed = 0;
            let mut error = None;
            while committed < input.bytes.len() {
                {
                    let mut state = lock(&shared.state);
                    if state.stop_write {
                        break;
                    }
                    state.observation.write_entered += 1;
                }
                let result = writer.write(&input.bytes[committed..]);
                lock(&shared.state).observation.write_returned += 1;
                match result {
                    Ok(0) => {
                        error = Some(Failure::io(
                            "write",
                            io::Error::from(io::ErrorKind::WriteZero),
                        ));
                        break;
                    }
                    Ok(count) => committed += count,
                    Err(failure) => {
                        error = Some(Failure::io("write", failure));
                        break;
                    }
                }
            }
            let mut state = lock(&shared.state);
            let outcome = if state.stop_write {
                WriteOutcome::Closing { committed, error }
            } else if let Some(error) = error {
                state.primary.get_or_insert(error.clone());
                state.cancel = true;
                state.stop_write = true;
                WriteOutcome::Failed { committed, error }
            } else {
                WriteOutcome::Complete(committed)
            };
            if state.stop_write {
                state.retired_input.push_back(input.bytes);
            } else {
                let length = input.bytes.len();
                drop(input.bytes);
                state.observation.input_bytes -= length;
                state.observation.input_items -= 1;
            }
            drop(state);
            let _ = input.reply.send(outcome);
            shared.signal();
        }
        drop(writer);
        lock(&shared.state).observation.writer_dropped = true;
    }
    shared.signal();
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct Hooks {
    pub before_initialize: Option<Arc<std::sync::Barrier>>,
    pub before_spawn: Option<Arc<std::sync::Barrier>>,
    pub fail_after_reader: bool,
    pub fail_tree: bool,
    pub fail_before_workers: bool,
    pub fail_after_reader_worker: bool,
    pub terminate_failures: Option<Arc<std::sync::atomic::AtomicUsize>>,
    pub library_panic_after_reader: Option<OsString>,
}
#[cfg(test)]
impl Hooks {
    fn before_spawn(&self) {
        if let Some(barrier) = &self.before_spawn {
            barrier.wait();
            barrier.wait();
        }
    }
}
#[cfg(test)]
impl LocalOwner {
    pub(crate) fn test_observer(&self) -> impl Fn() -> Observation + Send + 'static + use<> {
        let shared = Arc::clone(&self.shared);
        move || lock(&shared.state).observation
    }
    pub(crate) fn start_test(
        launch: Launch,
        term: OsString,
        geometry: Geometry,
        hooks: Hooks,
    ) -> Result<Self, Failure> {
        Self::start_inner(launch, term, geometry, hooks)
    }
}
