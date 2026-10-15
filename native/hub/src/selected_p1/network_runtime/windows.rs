use std::mem::{MaybeUninit, size_of};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, FIONBIO, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0, INVALID_SOCKET,
    IPPROTO_TCP, POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, POLLWRNORM, SD_BOTH, SO_ERROR, SO_SNDBUF,
    SOCK_STREAM, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6, SOCKADDR_IN6_0, SOCKADDR_STORAGE, SOCKET,
    SOCKET_ERROR, SOL_SOCKET, WSACleanup, WSADATA, WSAEINPROGRESS, WSAEWOULDBLOCK, WSAGetLastError,
    WSAPOLLFD, WSAPoll, WSAStartup, closesocket, connect, getpeername, getsockname, getsockopt,
    ioctlsocket, recv, send, setsockopt, shutdown, socket,
};
use windows_sys::Win32::System::Threading::Sleep;

use super::super::{RuntimeFault, Site};
use super::{BATCH, ERROR, PollEntry, READ, WRITE};

fn error(site: Site) -> RuntimeFault {
    // SAFETY: 当前线程刚返回 SOCKET_ERROR，不跨线程读取错误槽。
    RuntimeFault::Os {
        site,
        code: unsafe { WSAGetLastError() },
    }
}

pub(crate) struct Startup;
impl Startup {
    pub fn new() -> Result<Self, RuntimeFault> {
        let mut data = MaybeUninit::<WSADATA>::uninit();
        // SAFETY: WSADATA 的完整可写栈空间；2.2 版本，不借用栈地址到返回之后。
        let code = unsafe { WSAStartup(0x202, data.as_mut_ptr()) };
        if code == 0 {
            Ok(Self)
        } else {
            Err(RuntimeFault::Os {
                site: Site::Binding,
                code,
            })
        }
    }
}
impl Drop for Startup {
    fn drop(&mut self) {
        // SAFETY: 每个已成功 Startup 恰好一次 cleanup；所有 socket 保有 binding 根。
        unsafe {
            WSACleanup();
        }
    }
}

pub(crate) struct Socket(SOCKET);
impl Socket {
    pub fn connect(address: SocketAddr) -> Result<(Self, bool), RuntimeFault> {
        let family = if address.is_ipv4() { AF_INET } else { AF_INET6 };
        // SAFETY: 已成功持有 WSAStartup，创建普通 TCP socket，不申请新增系统权限。
        let raw = unsafe { socket(i32::from(family), SOCK_STREAM, IPPROTO_TCP) };
        if raw == INVALID_SOCKET {
            return Err(error(Site::Socket));
        }
        let socket = Self(raw);
        let mut nonblocking = 1;
        // SAFETY: raw 是存活 socket，参数为有效 u32 指针。
        if unsafe { ioctlsocket(raw, FIONBIO, &mut nonblocking) } == SOCKET_ERROR {
            return Err(error(Site::Socket));
        }
        let result = match address {
            SocketAddr::V4(address) => {
                let sockaddr = SOCKADDR_IN {
                    sin_family: AF_INET,
                    sin_port: address.port().to_be(),
                    sin_addr: IN_ADDR {
                        S_un: IN_ADDR_0 {
                            S_addr: u32::from_ne_bytes(address.ip().octets()),
                        },
                    },
                    sin_zero: [0; 8],
                };
                // SAFETY: 传入实际 IPv4 sockaddr 布局，Winsock 在返回前消费地址。
                unsafe {
                    connect(
                        raw,
                        (&sockaddr as *const SOCKADDR_IN).cast(),
                        size_of::<SOCKADDR_IN>() as i32,
                    )
                }
            }
            SocketAddr::V6(address) => {
                let sockaddr = SOCKADDR_IN6 {
                    sin6_family: AF_INET6,
                    sin6_port: address.port().to_be(),
                    sin6_flowinfo: address.flowinfo().to_be(),
                    sin6_addr: IN6_ADDR {
                        u: IN6_ADDR_0 {
                            Byte: address.ip().octets(),
                        },
                    },
                    Anonymous: SOCKADDR_IN6_0 {
                        sin6_scope_id: address.scope_id(),
                    },
                };
                // SAFETY: 同上，实际 IPv6 布局。
                unsafe {
                    connect(
                        raw,
                        (&sockaddr as *const SOCKADDR_IN6).cast(),
                        size_of::<SOCKADDR_IN6>() as i32,
                    )
                }
            }
        };
        if result == 0 {
            return Ok((socket, true));
        }
        let fault = error(Site::Socket);
        if matches!(
            fault,
            RuntimeFault::Os {
                code: WSAEWOULDBLOCK | WSAEINPROGRESS,
                ..
            }
        ) {
            Ok((socket, false))
        } else {
            Err(fault)
        }
    }

    pub fn raw(&self) -> usize {
        self.0
    }

    pub fn finish_connect(&self) -> Result<super::super::stream::SocketPeer, RuntimeFault> {
        let mut code = 0i32;
        let mut length = size_of::<i32>() as i32;
        // SAFETY: 存活 socket 和完整 i32 输出空间；仅在 connect readiness 后调用。
        if unsafe {
            getsockopt(
                self.0,
                SOL_SOCKET,
                SO_ERROR,
                (&mut code as *mut i32).cast(),
                &mut length,
            )
        } == SOCKET_ERROR
        {
            return Err(error(Site::Socket));
        }
        if code != 0 {
            return Err(RuntimeFault::Os {
                site: Site::Socket,
                code,
            });
        }
        Ok(super::super::stream::SocketPeer {
            local: self.address(false)?,
            peer: self.address(true)?,
        })
    }

    fn address(&self, peer: bool) -> Result<SocketAddr, RuntimeFault> {
        let mut storage = MaybeUninit::<SOCKADDR_STORAGE>::zeroed();
        let mut length = size_of::<SOCKADDR_STORAGE>() as i32;
        // SAFETY: sockaddr_storage 可容纳 v4/v6，先全部初始化；API 不保留指针。
        let status = unsafe {
            if peer {
                getpeername(self.0, storage.as_mut_ptr().cast::<SOCKADDR>(), &mut length)
            } else {
                getsockname(self.0, storage.as_mut_ptr().cast::<SOCKADDR>(), &mut length)
            }
        };
        if status == SOCKET_ERROR {
            return Err(error(Site::Socket));
        }
        let storage = unsafe { storage.assume_init() };
        match storage.ss_family {
            AF_INET if length as usize >= size_of::<SOCKADDR_IN>() => {
                // SAFETY: family、长度及 storage 对齐保证实际 sockaddr_in。
                let address =
                    unsafe { &*(&storage as *const SOCKADDR_STORAGE).cast::<SOCKADDR_IN>() };
                let octets = unsafe { address.sin_addr.S_un.S_addr }.to_ne_bytes();
                Ok(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::from(octets),
                    u16::from_be(address.sin_port),
                )))
            }
            AF_INET6 if length as usize >= size_of::<SOCKADDR_IN6>() => {
                let address =
                    unsafe { &*(&storage as *const SOCKADDR_STORAGE).cast::<SOCKADDR_IN6>() };
                Ok(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(unsafe { address.sin6_addr.u.Byte }),
                    u16::from_be(address.sin6_port),
                    u32::from_be(address.sin6_flowinfo),
                    unsafe { address.Anonymous.sin6_scope_id },
                )))
            }
            _ => Err(RuntimeFault::Os {
                site: Site::Socket,
                code: 10047,
            }),
        }
    }

    pub fn read(&self, bytes: &mut [u8]) -> Result<Option<usize>, RuntimeFault> {
        // SAFETY: 调用者传入已初始化且有界的可写 slice，非阻塞 socket。
        let count = unsafe { recv(self.0, bytes.as_mut_ptr(), bytes.len() as i32, 0) };
        self.count(count)
    }
    pub fn write(&self, bytes: &[u8]) -> Result<Option<usize>, RuntimeFault> {
        // SAFETY: 调用者传入有界的 slice，API 不保留指针。
        let count = unsafe { send(self.0, bytes.as_ptr(), bytes.len() as i32, 0) };
        self.count(count)
    }
    fn count(&self, count: i32) -> Result<Option<usize>, RuntimeFault> {
        if count >= 0 {
            return Ok(Some(count as usize));
        }
        let fault = error(Site::Socket);
        if matches!(
            fault,
            RuntimeFault::Os {
                code: WSAEWOULDBLOCK,
                ..
            }
        ) {
            Ok(None)
        } else {
            Err(fault)
        }
    }
    pub fn shutdown(&self) {
        // SAFETY: 活 socket；shutdown 不当作 closesocket/free/refund。
        unsafe {
            shutdown(self.0, SD_BOTH);
        }
    }
    pub fn send_buffer(&self, size: i32) -> Result<(), RuntimeFault> {
        // SAFETY: 实际 i32 参数，不影响 Rust backing 额度。
        if unsafe {
            setsockopt(
                self.0,
                SOL_SOCKET,
                SO_SNDBUF,
                (&size as *const i32).cast(),
                size_of::<i32>() as i32,
            )
        } == SOCKET_ERROR
        {
            Err(error(Site::Socket))
        } else {
            Ok(())
        }
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: unique ownership，没有复制 raw socket 或 from_raw 的别名 owner。
        unsafe {
            closesocket(self.0);
        }
    }
}

pub(crate) fn wait(duration: Duration) {
    // SAFETY: 只阻塞 poller 调用线程，不注册 Tokio/线程/TLS/回调。
    unsafe {
        Sleep(duration.as_millis().min(u32::MAX as u128) as u32);
    }
}

pub(crate) fn poll(entries: &mut [PollEntry], duration: Duration) -> Result<(), RuntimeFault> {
    let mut fds: [WSAPOLLFD; BATCH] = std::array::from_fn(|_| WSAPOLLFD {
        fd: INVALID_SOCKET,
        events: 0,
        revents: 0,
    });
    for (fd, entry) in fds.iter_mut().zip(entries.iter()) {
        fd.fd = entry.raw;
        if entry.interest & READ != 0 {
            fd.events |= POLLRDNORM;
        }
        if entry.interest & WRITE != 0 {
            fd.events |= POLLWRNORM;
        }
    }
    if entries.is_empty() {
        wait(duration);
        return Ok(());
    }
    // SAFETY: 不超过 BATCH 个 initialized fds；每个 snapshot 保持 socket 根到调用返回。
    let status = unsafe {
        WSAPoll(
            fds.as_mut_ptr(),
            entries.len() as u32,
            duration.as_millis().min(i32::MAX as u128) as i32,
        )
    };
    if status == SOCKET_ERROR {
        return Err(error(Site::Poller));
    }
    for (fd, entry) in fds.iter().zip(entries.iter_mut()) {
        if fd.revents & POLLRDNORM != 0 {
            entry.ready |= READ;
        }
        if fd.revents & POLLWRNORM != 0 {
            entry.ready |= WRITE;
        }
        if fd.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            entry.ready |= ERROR;
        }
    }
    Ok(())
}
