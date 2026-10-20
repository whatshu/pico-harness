use crate::{runtime::Runtime, session::Session};
use std::{
    collections::HashMap,
    io,
    net::{Shutdown, TcpListener, TcpStream},
    sync::{atomic::Ordering, Arc, Mutex},
    thread,
    time::Duration,
};

/// Bounded connection count limits the memory/thread cost of idle peers.
/// Shutting down the listener also closes every session, releasing leases.
pub fn serve(listener: TcpListener, runtime: Arc<Runtime>) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let connections = Arc::new(Mutex::new(HashMap::<u64, TcpStream>::new()));
    let mut workers = Vec::new();
    let mut next = 0u64;
    let pump = runtime.pump();
    let mut failure = None;
    while !runtime.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut registry = connections
                    .lock()
                    .map_err(|_| io::Error::other("session registry poisoned"))?;
                if registry.len() >= 64 {
                    drop(stream);
                    continue;
                }
                next += 1;
                let id = next;
                registry.insert(id, stream.try_clone()?);
                drop(registry);
                let registry = connections.clone();
                let runtime = runtime.clone();
                workers.push(thread::spawn(move || {
                    if let Err(error) = Session::new(id, runtime).run(stream) {
                        eprintln!("phd session {id}: {error}");
                    }
                    if let Ok(mut registry) = registry.lock() {
                        registry.remove(&id);
                    }
                }));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                runtime.stop.store(true, Ordering::Relaxed);
                failure = Some(error);
            }
        }
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }
    if let Ok(registry) = connections.lock() {
        for stream in registry.values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
    if let Ok(mut tasks) = runtime.tasks.lock() {
        tasks.shutdown();
    }
    let _ = pump.join();
    failure.map_or(Ok(()), Err)
}
