//! Bounded UDP receive prefetch; protocol handlers still run once per packet. POPSW-12.
use std::io;
use std::net::{SocketAddr, UdpSocket};

const BATCH: usize = 16;

pub(crate) struct UdpReceive {
    #[cfg(target_os = "linux")]
    batch: Option<LinuxBatch>,
    calls: u64,
    packets: u64,
    label: &'static str,
    port: u16,
}

impl UdpReceive {
    pub(crate) fn new(capacity: usize, label: &'static str, port: u16) -> Self {
        let enabled = !matches!(
            std::env::var("OPENNOW_RECV_BATCH").as_deref(),
            Ok("0" | "off" | "false" | "no")
        );
        let receiver = Self::with_mode(capacity, enabled, label, port);
        #[cfg(target_os = "linux")]
        let active = receiver.batch.is_some();
        #[cfg(not(target_os = "linux"))]
        let active = false;
        opennow_streamer_protocol::rlog!(
            Info,
            "udp_receive_batch",
            "socket={label} port={port} requested={enabled} active={active} max_packets={BATCH} packet_capacity={capacity}"
        );
        receiver
    }

    fn with_mode(capacity: usize, enabled: bool, label: &'static str, port: u16) -> Self {
        #[cfg(not(target_os = "linux"))]
        let _ = (capacity, enabled);
        Self {
            #[cfg(target_os = "linux")]
            batch: enabled.then(|| LinuxBatch::new(capacity).ok()).flatten(),
            calls: 0,
            packets: 0,
            label,
            port,
        }
    }

    pub(crate) fn recv_from(
        &mut self,
        socket: &UdpSocket,
        output: &mut Vec<u8>,
    ) -> io::Result<(usize, SocketAddr)> {
        #[cfg(target_os = "linux")]
        if let Some(batch) = self.batch.as_mut() {
            match batch.receive(socket, output, &mut self.calls) {
                Ok(value) => {
                    self.packets += 1;
                    return Ok(value);
                }
                Err(error)
                    if matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP)) =>
                {
                    self.batch = None;
                    let label = self.label;
                    let port = self.port;
                    opennow_streamer_protocol::rlog!(
                        Warn,
                        "udp_receive_batch_refused",
                        "socket={label} port={port} error={error} next=recvfrom"
                    );
                }
                Err(error) => return Err(error),
            }
        }
        self.calls += 1;
        let value = socket.recv_from(output)?;
        self.packets += 1;
        Ok(value)
    }
}

impl Drop for UdpReceive {
    fn drop(&mut self) {
        let label = self.label;
        let port = self.port;
        let calls = self.calls;
        let packets = self.packets;
        opennow_streamer_protocol::rlog!(
            Info,
            "udp_receive_batch_totals",
            "socket={label} port={port} calls={calls} packets={packets}"
        );
    }
}

#[cfg(target_os = "linux")]
struct LinuxBatch {
    buffers: Vec<Vec<u8>>,
    lengths: [usize; BATCH],
    sources: [SocketAddr; BATCH],
    capacity: usize,
    next: usize,
    count: usize,
}

#[cfg(target_os = "linux")]
impl LinuxBatch {
    fn new(capacity: usize) -> io::Result<Self> {
        if !(1..=65_536).contains(&capacity) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid receive capacity",
            ));
        }
        let mut buffers = Vec::new();
        buffers.try_reserve_exact(BATCH).map_err(io::Error::other)?;
        for _ in 0..BATCH {
            let mut buffer = Vec::new();
            buffer
                .try_reserve_exact(capacity)
                .map_err(io::Error::other)?;
            buffer.resize(capacity, 0);
            buffers.push(buffer);
        }
        Ok(Self {
            buffers,
            lengths: [0; BATCH],
            sources: [SocketAddr::from(([0, 0, 0, 0], 0)); BATCH],
            capacity,
            next: 0,
            count: 0,
        })
    }

    fn receive(
        &mut self,
        socket: &UdpSocket,
        output: &mut Vec<u8>,
        calls: &mut u64,
    ) -> io::Result<(usize, SocketAddr)> {
        if output.len() != self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "receive buffer size changed",
            ));
        }
        if self.next == self.count {
            self.refill(socket, calls)?;
        }
        let index = self.next;
        self.next += 1;
        // Descriptors only exist inside refill. Swapping owned buffers avoids a per-packet copy.
        std::mem::swap(output, &mut self.buffers[index]);
        Ok((self.lengths[index], self.sources[index]))
    }

    fn refill(&mut self, socket: &UdpSocket, calls: &mut u64) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let mut addresses: [libc::sockaddr_storage; BATCH] =
            std::array::from_fn(|_| unsafe { std::mem::zeroed() });
        let mut vectors: [libc::iovec; BATCH] =
            std::array::from_fn(|_| unsafe { std::mem::zeroed() });
        let mut messages: [libc::mmsghdr; BATCH] =
            std::array::from_fn(|_| unsafe { std::mem::zeroed() });
        for i in 0..BATCH {
            vectors[i].iov_base = self.buffers[i].as_mut_ptr().cast();
            vectors[i].iov_len = self.capacity;
            messages[i].msg_hdr.msg_iov = &mut vectors[i];
            messages[i].msg_hdr.msg_iovlen = 1;
            messages[i].msg_hdr.msg_name =
                (&mut addresses[i] as *mut libc::sockaddr_storage).cast();
            messages[i].msg_hdr.msg_namelen =
                std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        }
        *calls += 1;
        // All pointers refer to initialized storage which stays fixed until this synchronous call returns.
        // WAITFORONE uses the existing socket timeout for the first packet, then never waits to fill a batch.
        let got = unsafe {
            libc::recvmmsg(
                socket.as_raw_fd(),
                messages.as_mut_ptr(),
                BATCH as u32,
                libc::MSG_WAITFORONE,
                std::ptr::null_mut(),
            )
        };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        if got == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.next = 0;
        self.count = got as usize;
        for i in 0..self.count {
            self.lengths[i] = (messages[i].msg_len as usize).min(self.capacity);
            // Kernel-provided address is bounded by the sockaddr_storage supplied above.
            let source =
                unsafe { socket2::SockAddr::new(addresses[i], messages[i].msg_hdr.msg_namelen) };
            self.sources[i] = source
                .as_socket()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-IP UDP source"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn exercise(enabled: bool, ipv6: bool) {
        let bind = if ipv6 { "[::1]:0" } else { "127.0.0.1:0" };
        let receiver = UdpSocket::bind(bind).unwrap();
        let sender = UdpSocket::bind(bind).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut read = UdpReceive::with_mode(2048, enabled, "test", 0);
        let mut output = vec![0; 2048];
        for i in 0..40_u8 {
            sender
                .send_to(&vec![i; usize::from(i)], receiver.local_addr().unwrap())
                .unwrap();
        }
        for i in 0..40_u8 {
            let (len, source) = read.recv_from(&receiver, &mut output).unwrap();
            assert_eq!(source, sender.local_addr().unwrap());
            assert_eq!(len, usize::from(i));
            assert!(output[..len].iter().all(|byte| *byte == i));
        }
        assert_eq!(read.packets, 40);
        #[cfg(target_os = "linux")]
        if enabled {
            assert!(
                read.calls <= 3,
                "burst must use batches: {} calls",
                read.calls
            );
        }
        receiver.set_nonblocking(true).unwrap();
        assert_eq!(
            read.recv_from(&receiver, &mut output).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn batch_preserves_burst_order_lengths_sources_and_zero_length() {
        exercise(true, false);
    }
    #[test]
    fn rollback_preserves_datagrams() {
        exercise(false, false);
    }
    #[test]
    fn ipv6_sources_are_preserved() {
        exercise(true, true);
    }
    #[test]
    fn one_packet_does_not_wait_for_batch_to_fill() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        sender
            .send_to(b"one", receiver.local_addr().unwrap())
            .unwrap();
        let mut read = UdpReceive::with_mode(1500, true, "test", 0);
        let mut output = vec![0; 1500];
        let start = Instant::now();
        assert_eq!(read.recv_from(&receiver, &mut output).unwrap().0, 3);
        assert!(start.elapsed() < Duration::from_millis(150));
        assert_eq!(&output[..3], b"one");
    }
    #[test]
    fn cached_packets_survive_caller_timer_and_socket_changes() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        for i in 0..8_u8 {
            sender
                .send_to(&[i], receiver.local_addr().unwrap())
                .unwrap();
        }
        let mut read = UdpReceive::with_mode(1500, true, "test", 0);
        let mut output = vec![0; 1500];
        read.recv_from(&receiver, &mut output).unwrap();
        receiver.set_nonblocking(true).unwrap();
        for i in 1..8_u8 {
            // The caller retains its loop turn between deliveries (timers/control/stop checks).
            receiver
                .set_read_timeout(Some(Duration::from_millis(1)))
                .unwrap();
            assert_eq!(read.recv_from(&receiver, &mut output).unwrap().0, 1);
            assert_eq!(output[0], i);
        }
        #[cfg(target_os = "linux")]
        assert_eq!(read.calls, 1);
    }
    #[test]
    fn idle_timeout_remains_an_idle_error() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let mut read = UdpReceive::with_mode(1500, true, "test", 0);
        let error = read.recv_from(&receiver, &mut vec![0; 1500]).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
    }
    #[test]
    fn mixed_senders_keep_each_datagrams_source() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let first = UdpSocket::bind("127.0.0.1:0").unwrap();
        let second = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        for i in 0..8_u8 {
            let sender = if i % 2 == 0 { &first } else { &second };
            sender
                .send_to(&[i], receiver.local_addr().unwrap())
                .unwrap();
        }
        let mut read = UdpReceive::with_mode(1500, true, "test", 0);
        let mut output = vec![0; 1500];
        for i in 0..8_u8 {
            let (len, source) = read.recv_from(&receiver, &mut output).unwrap();
            assert_eq!(len, 1);
            assert_eq!(output[0], i);
            assert_eq!(
                source,
                if i % 2 == 0 {
                    first.local_addr().unwrap()
                } else {
                    second.local_addr().unwrap()
                }
            );
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn maximum_udp_payload_and_following_packet_stay_intact() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        sender
            .send_to(&vec![0x42; 65_507], receiver.local_addr().unwrap())
            .unwrap();
        sender
            .send_to(b"next", receiver.local_addr().unwrap())
            .unwrap();
        let mut read = UdpReceive::with_mode(65_536, true, "test", 0);
        let mut output = vec![0; 65_536];
        let (len, _) = read.recv_from(&receiver, &mut output).unwrap();
        assert_eq!(len, 65_507);
        assert!(output[..len].iter().all(|byte| *byte == 0x42));
        assert_eq!(read.recv_from(&receiver, &mut output).unwrap().0, 4);
        assert_eq!(&output[..4], b"next");
    }
}
