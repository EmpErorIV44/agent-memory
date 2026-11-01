//! Private, bounded local daemon IPC. The retained lock file is never unlinked.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Request {
    fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.command.as_str(), "status" | "stop" | "refresh"),
            "unsupported daemon command"
        );
        if let Some(project) = &self.project {
            memory_core::validate_document_id(project)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use anyhow::{bail, Context};
    use cap_primitives::fs::OpenOptionsExt;
    use cap_std::{
        ambient_authority,
        fs::{Dir, DirBuilderExt, FileTypeExt, MetadataExt as CapMetadataExt},
    };
    use std::{
        fs::File,
        io::{ErrorKind, Read, Write},
        os::unix::{
            fs::{MetadataExt, PermissionsExt},
            io::{AsRawFd, FromRawFd},
            net::{UnixListener, UnixStream},
        },
        path::{Component, PathBuf},
        time::{Duration, Instant},
    };

    const SOCKET: &str = "control.sock";
    const LOCK: &str = "owner.lock";
    const MAX_BYTES: usize = 64 * 1024;
    const SERVER_READ: Duration = Duration::from_secs(1);
    const CLIENT_IO: Duration = Duration::from_secs(5);

    fn euid() -> u32 {
        // SAFETY: geteuid takes no pointers and only reads process credentials.
        unsafe { libc::geteuid() }
    }

    fn open_directory(path: &Path) -> Result<Dir> {
        ensure!(path.is_absolute(), "daemon directory must be absolute");
        let mut directory = Dir::open_ambient_dir("/", ambient_authority())?.into_std_file();
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(name))
                        .context("daemon path refuses symlinks")?;
                }
                _ => bail!("daemon directory must be canonical"),
            }
        }
        Ok(Dir::from_std_file(directory))
    }

    fn runtime(database: &Path, create: bool) -> Result<(PathBuf, Dir)> {
        let database = database
            .canonicalize()
            .context("resolve existing daemon database")?;
        let database_metadata = std::fs::symlink_metadata(&database)?;
        ensure!(
            database_metadata.is_file() && database_metadata.nlink() == 1,
            "daemon database must be a regular file with one hard link"
        );
        let (parent_path, parent, name) = {
            // A short OS runtime path avoids Unix socket length limits and does
            // not depend on POSIX modes being supported by the database filesystem.
            #[cfg(target_os = "macos")]
            let temporary_path = PathBuf::from("/private/tmp");
            #[cfg(not(target_os = "macos"))]
            let temporary_path = Path::new("/tmp").canonicalize()?;
            let temporary = open_directory(&temporary_path)?;
            let base = format!(".agent-memory-daemon-{}", euid());
            let parent = private_child(&temporary, Path::new(&base), create)?;
            let parent_path = temporary_path.join(base);
            let key = memory_core::content_hash(&database_identity(&database)?);
            (parent_path, parent, std::ffi::OsString::from(&key[..32]))
        };
        let directory = private_child(&parent, Path::new(&name), create)?;
        Ok((parent_path.join(name), directory))
    }

    fn private_child(parent: &Dir, name: &Path, create: bool) -> Result<Dir> {
        if create {
            let mut builder = cap_std::fs::DirBuilder::new();
            builder.mode(0o700);
            match parent.create_dir_with(name, &builder) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        let anchor = parent.try_clone()?.into_std_file();
        let handle = cap_primitives::fs::open_dir_nofollow(&anchor, name)
            .context("open private daemon runtime directory")?;
        let metadata = handle.metadata()?;
        ensure!(
            metadata.uid() == euid() && metadata.mode() & 0o777 == 0o700,
            "daemon runtime directory must be owner-owned with mode 0700"
        );
        Ok(Dir::from_std_file(handle))
    }

    fn endpoint_for_io(directory: &Dir, _displayed: &Path) -> Result<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            // std has no bindat/connectat; /proc resolves this pinned directory fd.
            Ok(PathBuf::from(format!(
                "/proc/self/fd/{}/{}",
                directory.as_raw_fd(),
                SOCKET
            )))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = directory;
            Ok(_displayed.to_path_buf())
        }
    }

    fn verify_runtime_path(path: &Path, dir: &Dir) -> Result<()> {
        let current = open_directory(path)?.into_std_file().metadata()?;
        let pinned = dir.try_clone()?.into_std_file().metadata()?;
        ensure!(
            (current.dev(), current.ino()) == (pinned.dev(), pinned.ino()),
            "daemon runtime directory was replaced"
        );
        Ok(())
    }

    fn socket_identity(dir: &Dir) -> Result<(u64, u64)> {
        socket_record(dir, true)
    }

    fn socket_record(dir: &Dir, require_private_mode: bool) -> Result<(u64, u64)> {
        let metadata = dir.symlink_metadata(SOCKET)?;
        ensure!(
            metadata.file_type().is_socket()
                && metadata.uid() == euid()
                && (!require_private_mode || metadata.mode() & 0o777 == 0o600),
            "daemon endpoint must be an owner-owned socket with mode 0600"
        );
        Ok((metadata.dev(), metadata.ino()))
    }

    fn database_identity(database: &Path) -> Result<Vec<u8>> {
        use std::os::unix::ffi::OsStrExt;
        let database = database.canonicalize()?;
        let metadata = std::fs::symlink_metadata(&database)?;
        ensure!(
            metadata.is_file() && metadata.nlink() == 1,
            "daemon database must be a regular file with one hard link"
        );
        Ok(serde_json::to_vec(&(
            database.as_os_str().as_bytes(),
            metadata.dev(),
            metadata.ino(),
        ))?)
    }

    fn verify_database_identity(database: &Path, dir: &Dir, create: bool) -> Result<()> {
        let expected = database_identity(database)?;
        ensure!(
            expected.len() <= 16384,
            "daemon database identity is too long"
        );
        let anchor = dir.try_clone()?.into_std_file();
        let mut options = cap_primitives::fs::OpenOptions::new();
        options
            .read(true)
            ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
        if create {
            publish_identity(dir, &expected, || Ok(()), || Ok(()))?;
        }
        let file = cap_primitives::fs::open(&anchor, Path::new("database.identity"), &options)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == euid()
                && metadata.mode() & 0o777 == 0o600
                && metadata.len() <= 16384,
            "invalid daemon database identity record"
        );
        let mut actual = Vec::new();
        file.take(16385).read_to_end(&mut actual)?;
        ensure!(
            actual == expected,
            "daemon runtime belongs to another database"
        );
        Ok(())
    }

    fn publish_identity(
        dir: &Dir,
        expected: &[u8],
        before: impl FnOnce() -> Result<()>,
        after: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        match dir.symlink_metadata("database.identity") {
            Ok(_) => return Ok(()), // Existing records are validated by the caller.
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let anchor = dir.try_clone()?.into_std_file();
        let mut options = cap_primitives::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
        let (temporary, mut file) = loop {
            let temporary = format!(
                ".identity-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            match cap_primitives::fs::open(&anchor, Path::new(&temporary), &options) {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let result = (|| -> Result<()> {
            file.write_all(expected)?;
            file.sync_all()?;
            before()?;
            match dir.hard_link(&temporary, dir, "database.identity") {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            after()?;
            dir.open(".")?.sync_all()?;
            Ok(())
        })();
        drop(file);
        let _ = dir.remove_file(&temporary);
        // A crash between link and cleanup may leave a second private hard link.
        // Readers validate immutable bytes and allow that harmless temporary residue.
        result
    }

    /// Nonblocking Unix connect with a deadline, including full-backlog EAGAIN.
    fn connect(path: &Path, timeout: Duration) -> Result<UnixStream> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: sockaddr_un is a C integer/byte structure; all-zero is valid initialization.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        ensure!(
            !bytes.contains(&0) && bytes.len() < address.sun_path.len(),
            "daemon socket path is too long or contains NUL"
        );
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (to, from) in address.sun_path.iter_mut().zip(bytes) {
            *to = *from as libc::c_char;
        }
        let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        ))]
        {
            address.sun_len = length as u8;
        }
        // SAFETY: socket returns a fresh owned descriptor or -1; no borrowed pointers.
        let socket_kind = libc::SOCK_STREAM;
        #[cfg(target_os = "linux")]
        let socket_kind = socket_kind | libc::SOCK_CLOEXEC;
        let fd = unsafe { libc::socket(libc::AF_UNIX, socket_kind, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fd is fresh and ownership is transferred exactly once.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        // SAFETY: fcntl changes only descriptor flags on this owned live socket.
        if unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        stream.set_nonblocking(true)?;
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: address is initialized and its bounded length fits sockaddr_un.
            let result = unsafe {
                libc::connect(
                    stream.as_raw_fd(),
                    (&address as *const libc::sockaddr_un).cast(),
                    length as libc::socklen_t,
                )
            };
            if result == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EISCONN) {
                break;
            }
            ensure!(Instant::now() < deadline, "daemon connect timed out");
            match error.raw_os_error() {
                Some(libc::EINPROGRESS) | Some(libc::EALREADY) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let mut poll = libc::pollfd {
                        fd: stream.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    // SAFETY: poll references exactly one valid pollfd during the call.
                    let ready = unsafe {
                        libc::poll(
                            &mut poll,
                            1,
                            remaining.as_millis().min(i32::MAX as u128) as i32,
                        )
                    };
                    if ready < 0 {
                        let error = std::io::Error::last_os_error();
                        if error.kind() == ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(error.into());
                    }
                    ensure!(ready > 0, "daemon connect timed out");
                    if let Some(error) = stream.take_error()? {
                        return Err(error.into());
                    }
                    break;
                }
                Some(libc::EAGAIN) | Some(libc::EINTR) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => return Err(error.into()),
            }
        }
        stream.set_nonblocking(false)?;
        Ok(stream)
    }

    fn read_frame(stream: &mut UnixStream, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("daemon request timed out")?;
            stream.set_read_timeout(Some(remaining))?;
            let mut chunk = [0; 4096];
            let count = match stream.read(&mut chunk) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                result => result.context("read daemon JSON frame")?,
            };
            ensure!(count != 0, "truncated daemon JSON frame");
            if let Some(end) = chunk[..count].iter().position(|b| *b == b'\n') {
                ensure!(
                    bytes.len() + end <= MAX_BYTES,
                    "daemon JSON frame exceeds 64 KiB"
                );
                ensure!(
                    chunk[end + 1..count].iter().all(u8::is_ascii_whitespace),
                    "multiple daemon JSON frames are not allowed"
                );
                bytes.extend_from_slice(&chunk[..end]);
                return serde_json::from_slice(&bytes).context("invalid daemon JSON frame");
            }
            bytes.extend_from_slice(&chunk[..count]);
            ensure!(bytes.len() <= MAX_BYTES, "daemon JSON frame exceeds 64 KiB");
        }
    }

    fn write_frame(stream: &mut UnixStream, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        ensure!(bytes.len() <= MAX_BYTES, "daemon JSON frame exceeds 64 KiB");
        bytes.push(b'\n');
        let deadline = Instant::now() + CLIENT_IO;
        let mut offset = 0;
        while offset < bytes.len() {
            stream.set_write_timeout(Some(
                deadline
                    .checked_duration_since(Instant::now())
                    .context("daemon response timed out")?,
            ))?;
            match stream.write(&bytes[offset..]) {
                Ok(0) => bail!("daemon connection closed during write"),
                Ok(count) => offset += count,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub struct Lease {
        listener: UnixListener,
        // Held until Drop has removed only the socket this lease created.
        _lock: File,
        directory: Dir,
        runtime: PathBuf,
        endpoint: PathBuf,
        socket_identity: (u64, u64),
    }

    impl Lease {
        pub fn acquire(database: &Path) -> Result<Self> {
            let (runtime, directory) = runtime(database, true)?;
            let anchor = directory.try_clone()?.into_std_file();
            let mut options = cap_primitives::fs::OpenOptions::new();
            options
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
            let lock = cap_primitives::fs::open(&anchor, Path::new(LOCK), &options)?;
            let metadata = lock.metadata()?;
            ensure!(
                metadata.is_file()
                    && metadata.nlink() == 1
                    && metadata.uid() == euid()
                    && metadata.mode() & 0o777 == 0o600,
                "daemon lock must be an owner-owned private regular file"
            );
            lock.try_lock()
                .context("another daemon owns this database")?;
            verify_database_identity(database, &directory, true)?;
            verify_runtime_path(&runtime, &directory)?;
            let endpoint = runtime.join(SOCKET);
            let io_endpoint = endpoint_for_io(&directory, &endpoint)?;
            match directory.symlink_metadata(SOCKET) {
                Ok(_) => {
                    // A killed startup can leave a socket before chmod(0600).
                    // Under the owned lock and private directory, only an owned,
                    // refused socket is stale; clients still require strict 0600.
                    socket_record(&directory, false)?;
                    match connect(&io_endpoint, Duration::from_secs(1)) {
                        Ok(_) => bail!("daemon endpoint is already active"),
                        Err(error)
                            if error
                                .downcast_ref::<std::io::Error>()
                                .is_some_and(|e| e.kind() == ErrorKind::ConnectionRefused) =>
                        {
                            directory.remove_file(SOCKET)?
                        }
                        Err(error) => {
                            return Err(error.context("refuse to remove unverified daemon endpoint"))
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let listener =
                UnixListener::bind(&io_endpoint).context("bind private daemon socket")?;
            // The parent directory is 0700 before bind, so even this initial mode gap is private.
            directory.set_permissions(
                SOCKET,
                cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o600)),
            )?;
            let socket_identity = socket_identity(&directory)?;
            listener.set_nonblocking(true)?;
            verify_runtime_path(&runtime, &directory)?;
            Ok(Self {
                listener,
                _lock: lock,
                directory,
                runtime,
                endpoint,
                socket_identity,
            })
        }

        pub fn endpoint(&self) -> &Path {
            &self.endpoint
        }

        pub fn accept(&self) -> Result<Option<Connection>> {
            verify_runtime_path(&self.runtime, &self.directory)?;
            ensure!(
                socket_identity(&self.directory)? == self.socket_identity,
                "daemon endpoint was replaced"
            );
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    Ok(Some(Connection { stream }))
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
    }

    impl Drop for Lease {
        fn drop(&mut self) {
            if socket_identity(&self.directory).ok() == Some(self.socket_identity) {
                let _ = self.directory.remove_file(SOCKET);
            }
            // owner.lock stays in place: unlinking it would permit split-inode locks.
        }
    }

    pub struct Connection {
        stream: UnixStream,
    }
    impl Connection {
        pub fn read_request(&mut self) -> Result<Request> {
            let request: Request =
                serde_json::from_value(read_frame(&mut self.stream, SERVER_READ)?)?;
            request.validate()?;
            Ok(request)
        }
        pub fn reply(&mut self, value: &Value) -> Result<()> {
            write_frame(&mut self.stream, value)
        }
    }

    pub fn request(database: &Path, command: &str, project: Option<&str>) -> Result<Value> {
        let request = Request {
            command: command.into(),
            project: project.map(str::to_owned),
        };
        request.validate()?;
        let (runtime, directory) = runtime(database, false)?;
        verify_database_identity(database, &directory, false)?;
        let identity = socket_identity(&directory)?;
        verify_runtime_path(&runtime, &directory)?;
        let mut stream = connect(
            &endpoint_for_io(&directory, &runtime.join(SOCKET))?,
            CLIENT_IO,
        )
        .context("connect to local daemon")?;
        ensure!(
            socket_identity(&directory)? == identity,
            "daemon endpoint changed during connect"
        );
        verify_runtime_path(&runtime, &directory)?;
        write_frame(&mut stream, &serde_json::to_value(request)?)?;
        read_frame(&mut stream, CLIENT_IO)
    }
    #[cfg(test)]
    mod identity_tests {
        use super::*;

        #[test]
        fn crash_child() -> Result<()> {
            let Some(database) = std::env::var_os("AGENT_MEMORY_DAEMON_IDENTITY_CRASH_DB") else {
                return Ok(());
            };
            let stage = std::env::var("AGENT_MEMORY_DAEMON_IDENTITY_CRASH_STAGE")?;
            let database = PathBuf::from(database);
            let (_path, dir) = runtime(&database, true)?;
            publish_identity(
                &dir,
                &database_identity(&database)?,
                || {
                    if stage == "before" {
                        std::process::exit(73);
                    }
                    Ok(())
                },
                || {
                    std::process::exit(73);
                },
            )?;
            anyhow::bail!("identity crash hook not reached")
        }

        #[test]
        fn abrupt_identity_publication_is_recoverable_before_and_after_link() -> Result<()> {
            for stage in ["before", "after"] {
                let temporary = tempfile::tempdir()?;
                let database = temporary.path().join("identity.sqlite");
                std::fs::write(&database, b"test")?;
                let child = std::process::Command::new(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "daemon_ipc::platform::identity_tests::crash_child",
                        "--nocapture",
                    ])
                    .env("AGENT_MEMORY_DAEMON_IDENTITY_CRASH_DB", &database)
                    .env("AGENT_MEMORY_DAEMON_IDENTITY_CRASH_STAGE", stage)
                    .output()?;
                ensure!(
                    child.status.code() == Some(73),
                    "identity child status {:?}: {}",
                    child.status,
                    String::from_utf8_lossy(&child.stderr)
                );
                let (runtime_path, _) = runtime(&database, false)?;
                assert_eq!(
                    runtime_path.join("database.identity").exists(),
                    stage == "after"
                );
                let lease = Lease::acquire(&database)?;
                verify_database_identity(&database, &lease.directory, false)?;
                drop(lease);
            }
            Ok(())
        }
    }
}

#[cfg(unix)]
pub use platform::{request, Lease};

#[cfg(not(unix))]
mod platform {
    use super::*;
    pub struct Lease;
    pub struct Connection;
    impl Lease {
        pub fn acquire(_: &Path) -> Result<Self> {
            anyhow::bail!("local daemon IPC requires Unix; use WSL on Windows")
        }
        pub fn endpoint(&self) -> &Path {
            Path::new("")
        }
        pub fn accept(&self) -> Result<Option<Connection>> {
            anyhow::bail!("local daemon IPC requires Unix")
        }
    }
    impl Connection {
        pub fn read_request(&mut self) -> Result<Request> {
            anyhow::bail!("local daemon IPC requires Unix")
        }
        pub fn reply(&mut self, _: &Value) -> Result<()> {
            anyhow::bail!("local daemon IPC requires Unix")
        }
    }
    pub fn request(_: &Path, _: &str, _: Option<&str>) -> Result<Value> {
        anyhow::bail!("local daemon IPC requires Unix; use WSL on Windows")
    }
}
#[cfg(not(unix))]
pub use platform::{request, Lease};

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use anyhow::Context;
    use serde_json::json;
    use std::{
        fs,
        io::Write,
        os::unix::{
            fs::{symlink, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        thread,
        time::{Duration, Instant},
    };

    fn database() -> Result<(tempfile::TempDir, std::path::PathBuf)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("memory.sqlite");
        fs::write(&path, b"test database identity")?;
        Ok((dir, path))
    }

    fn accept(lease: &Lease) -> Result<super::platform::Connection> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(connection) = lease.accept()? {
                return Ok(connection);
            }
            anyhow::ensure!(Instant::now() < deadline, "test accept timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn real_ipc_is_private_and_singleton_across_database_symlink_aliases() -> Result<()> {
        let (dir, database) = database()?;
        let lease = Lease::acquire(&database)?;
        let endpoint = lease.endpoint().to_path_buf();
        assert_eq!(
            fs::metadata(endpoint.parent().unwrap())?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(fs::metadata(&endpoint)?.permissions().mode() & 0o777, 0o600);
        let alias = dir.path().join("alias.sqlite");
        symlink(&database, &alias)?;
        assert!(Lease::acquire(&alias).is_err());
        let server = thread::spawn(move || -> Result<()> {
            let mut connection = accept(&lease)?;
            let request = connection.read_request()?;
            assert_eq!(request.command, "refresh");
            assert_eq!(request.project.as_deref(), Some("demo"));
            connection.reply(&json!({"ok":true,"indexed":true}))
        });
        assert_eq!(request(&alias, "refresh", Some("demo"))?["indexed"], true);
        server.join().unwrap()?;
        assert!(!endpoint.exists());
        assert!(endpoint.parent().unwrap().join("owner.lock").is_file());
        let replacement = Lease::acquire(&database)?;
        drop(replacement);
        Ok(())
    }

    #[test]
    fn stale_socket_recovery_refuses_live_collisions_and_regular_files() -> Result<()> {
        let (_dir, database) = database()?;
        let lease = Lease::acquire(&database)?;
        let endpoint = lease.endpoint().to_path_buf();
        drop(lease);
        let other = UnixListener::bind(&endpoint)?;
        // Simulate interruption after bind but before the daemon's chmod(0600).
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o755))?;
        assert!(Lease::acquire(&database).is_err());
        assert!(endpoint.exists());
        assert_eq!(fs::metadata(&endpoint)?.permissions().mode() & 0o777, 0o755);
        drop(other);
        let recovered = Lease::acquire(&database)?;
        assert_eq!(
            fs::metadata(recovered.endpoint())?.permissions().mode() & 0o777,
            0o600
        );
        drop(recovered);
        fs::write(&endpoint, b"must survive")?;
        assert!(Lease::acquire(&database).is_err());
        assert_eq!(fs::read(&endpoint)?, b"must survive");
        Ok(())
    }

    #[test]
    fn runtime_lock_socket_and_identity_symlinks_are_rejected() -> Result<()> {
        for component in ["owner.lock", "control.sock", "database.identity"] {
            let (dir, database) = database()?;
            let lease = Lease::acquire(&database)?;
            let runtime = lease.endpoint().parent().unwrap().to_path_buf();
            drop(lease);
            let target = dir.path().join("outside");
            fs::write(&target, b"unchanged")?;
            let path = runtime.join(component);
            if path.exists() {
                fs::remove_file(&path)?;
            }
            symlink(&target, &path)?;
            assert!(
                Lease::acquire(&database).is_err(),
                "accepted symlink {component}"
            );
            assert!(request(&database, "status", None).is_err());
            assert_eq!(fs::read(target)?, b"unchanged");
        }
        let (dir, database) = database()?;
        let lease = Lease::acquire(&database)?;
        let runtime = lease.endpoint().parent().unwrap().to_path_buf();
        drop(lease);
        fs::remove_dir_all(&runtime)?;
        let outside = dir.path().join("outside-dir");
        fs::create_dir(&outside)?;
        symlink(&outside, &runtime)?;
        assert!(Lease::acquire(&database).is_err());
        assert!(fs::read_dir(&outside)?.next().is_none());
        Ok(())
    }

    #[test]
    fn permissive_runtime_and_hardlinked_databases_fail_closed() -> Result<()> {
        let (dir, database) = database()?;
        let lease = Lease::acquire(&database)?;
        let runtime = lease.endpoint().parent().unwrap().to_path_buf();
        drop(lease);
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755))?;
        assert!(Lease::acquire(&database).is_err());
        assert_eq!(fs::metadata(&runtime)?.permissions().mode() & 0o777, 0o755);
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        fs::hard_link(&database, dir.path().join("hardlink.sqlite"))?;
        assert!(Lease::acquire(&database).is_err());
        Ok(())
    }

    #[test]
    fn malformed_oversized_and_unknown_requests_are_bounded_and_do_not_poison_listener(
    ) -> Result<()> {
        let (_dir, database) = database()?;
        let lease = Lease::acquire(&database)?;
        for frame in [
            b"not-json\n".to_vec(),
            b"{\"command\":\"shell\"}\n".to_vec(),
            b"{\"command\":\"status\",\"unexpected\":true}\n".to_vec(),
            vec![b'x'; 64 * 1024 + 2],
        ] {
            let mut peer = UnixStream::connect(lease.endpoint())?;
            let sender = thread::spawn(move || {
                let _ = peer.write_all(&frame);
            });
            let mut connection = accept(&lease)?;
            assert!(connection.read_request().is_err());
            drop(connection);
            sender.join().unwrap();
        }
        let server = thread::spawn(move || -> Result<()> {
            let mut connection = accept(&lease)?;
            assert_eq!(connection.read_request()?.command, "status");
            connection.reply(&json!({"running":true}))
        });
        assert_eq!(request(&database, "status", None)?["running"], true);
        server.join().unwrap()?;
        Ok(())
    }

    #[test]
    fn idle_clients_have_an_absolute_one_second_request_deadline() -> Result<()> {
        let (_dir, database) = database()?;
        let lease = Lease::acquire(&database)?;
        let _peer = UnixStream::connect(lease.endpoint())?;
        let mut connection = accept(&lease)?;
        let started = Instant::now();
        assert!(connection.read_request().is_err());
        assert!(started.elapsed() < Duration::from_secs(4));
        Ok(())
    }

    #[test]
    fn deeply_nested_database_paths_work_without_unix_socket_path_overflow() -> Result<()> {
        let (dir, _database) = database()?;
        let nested = dir.path().join("long-directory-component".repeat(7));
        fs::create_dir(&nested)?;
        let database = nested.join("database.sqlite");
        fs::write(&database, b"identity")?;
        let lease =
            Lease::acquire(&database).context("deep database path must support daemon IPC")?;
        let server = thread::spawn(move || -> Result<()> {
            let mut connection = accept(&lease)?;
            connection.read_request()?;
            connection.reply(&json!({"running":true}))
        });
        assert_eq!(request(&database, "status", None)?["running"], true);
        server.join().unwrap()?;
        Ok(())
    }
}
