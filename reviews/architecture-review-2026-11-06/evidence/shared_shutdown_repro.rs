use std::{io::{Read, Write}, net::{TcpListener, TcpStream, Shutdown}, sync::{mpsc, Arc}, thread, time::{Instant, Duration}};
fn main() {
    let mut worst = Duration::ZERO;
    for trial in 1..=5000 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready, seen_ready) = mpsc::channel();
        let (closed, seen_close) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            ready.send(()).unwrap();
            let result = socket.read(&mut byte).map_err(|e| e.kind());
            closed.send(result).unwrap();
        });
        let socket = Arc::new(TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap());
        socket.set_nodelay(true).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let control = socket.clone();
        let worker = thread::spawn(move || {
            let mut stream = &*socket;
            stream.write_all(b"CONNECT fixture.invalid:80 HTTP/1.1\r\nHost: fixture.invalid:80\r\n\r\n").unwrap();
            let _ = stream.read_exact(&mut [0]);
        });
        seen_ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let shutdown = control.shutdown(Shutdown::Both);
        drop(control);
        let result = seen_close.recv_timeout(Duration::from_secs(1));
        worst = worst.max(started.elapsed());
        if result.is_err() {
            println!("FAIL trial={trial} shutdown={shutdown:?} result={result:?} later={:?}", seen_close.recv_timeout(Duration::from_secs(5)));
            server.join().unwrap(); worker.join().unwrap();
            std::process::exit(1);
        }
        server.join().unwrap(); worker.join().unwrap();
    }
    println!("PASS 5000 shared-socket cancellations, maximum {worst:?}");
}
