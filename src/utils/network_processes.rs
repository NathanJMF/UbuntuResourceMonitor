use std::{collections::HashMap, net::IpAddr, process::Command, time::Instant};

pub type InterfaceRates = HashMap<String, HashMap<i32, (f64, f64)>>;

#[derive(Debug, PartialEq, Eq)]
struct Socket {
    cookie: String,
    local: IpAddr,
    remote: IpAddr,
    pid: i32,
    received: u64,
    sent: u64,
}

fn normalized(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        _ => address,
    }
}

fn parse_socket(line: &str) -> Option<Socket> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "ESTAB" {
        return None;
    }
    let parse_address = |endpoint: &str| {
        let address = endpoint.rsplit_once(':')?.0.trim_matches(['[', ']']);
        address.split('%').next()?.parse().ok().map(normalized)
    };
    let local = parse_address(fields.nth(2)?)?;
    let remote = parse_address(fields.next()?)?;
    let owners = line.split_once("users:(")?.1;
    let mut pids = owners
        .split("pid=")
        .skip(1)
        .filter_map(|owner| owner.split(',').next()?.parse::<i32>().ok());
    let pid = pids.next()?;
    if pids.any(|other| other != pid) {
        return None;
    }
    let token = |prefix| {
        line.split_whitespace()
            .find_map(|word| word.strip_prefix(prefix))
    };
    let number = |prefix| {
        token(prefix)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    };
    Some(Socket {
        cookie: token("sk:")?.to_owned(),
        local,
        remote,
        pid,
        received: number("bytes_received:"),
        sent: number("bytes_acked:"),
    })
}

#[derive(Default)]
pub struct Sampler {
    previous: HashMap<String, Socket>,
    last_sample: Option<Instant>,
}

impl Sampler {
    pub fn sample(&mut self, enabled: bool) -> InterfaceRates {
        if !enabled {
            self.previous.clear();
            self.last_sample = None;
            return HashMap::new();
        }

        let Ok(output) = Command::new("ss")
            .args(["-H", "-O", "-n", "-t", "-i", "-p", "-e"])
            .output()
        else {
            return HashMap::new();
        };
        if !output.status.success() {
            return HashMap::new();
        }

        let now = Instant::now();
        let elapsed = self
            .last_sample
            .replace(now)
            .map(|time| now.duration_since(time).as_secs_f64());
        let current: HashMap<_, _> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_socket)
            .map(|socket| (socket.cookie.clone(), socket))
            .collect();
        let previous = std::mem::replace(&mut self.previous, current);
        let Some(elapsed) = elapsed.filter(|elapsed| *elapsed > 0.0) else {
            return HashMap::new();
        };

        let mut interfaces = HashMap::<IpAddr, Option<String>>::new();
        if let Ok(addresses) = nix::ifaddrs::getifaddrs() {
            for interface in addresses {
                let Some(address) = interface.address else {
                    continue;
                };
                let address = address
                    .as_sockaddr_in()
                    .map(|address| IpAddr::V4(address.ip()))
                    .or_else(|| {
                        address
                            .as_sockaddr_in6()
                            .map(|address| IpAddr::V6(address.ip()))
                    });
                if let Some(address) = address {
                    let name = interface.interface_name;
                    interfaces
                        .entry(normalized(address))
                        .and_modify(|known| {
                            if known.as_ref() != Some(&name) {
                                *known = None;
                            }
                        })
                        .or_insert(Some(name));
                }
            }
        }

        let mut rates = InterfaceRates::new();
        for (cookie, socket) in &self.previous {
            let Some(old) = previous
                .get(cookie)
                .filter(|old| old.pid == socket.pid && old.local == socket.local)
            else {
                continue;
            };
            let Some(Some(interface)) = interfaces.get(&socket.local) else {
                continue;
            };
            // Local connections can use a physical interface's address while
            // their packets stay on the host.
            if interface != "lo" && interfaces.contains_key(&socket.remote) {
                continue;
            }
            let received = socket.received.saturating_sub(old.received);
            let sent = socket.sent.saturating_sub(old.sent);
            if received != 0 || sent != 0 {
                let rate = rates
                    .entry(interface.clone())
                    .or_default()
                    .entry(socket.pid)
                    .or_default();
                rate.0 += received as f64 / elapsed;
                rate.1 += sent as f64 / elapsed;
            }
        }
        rates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_socket_counters_and_owner() {
        let line = "ESTAB 0 0 [::ffff:192.168.0.2]:443 192.168.0.3:42100 users:((\"example\",pid=42,fd=5)) uid:1000 ino:8 sk:a <-> cubic bytes_acked:180 bytes_received:250";
        let socket = parse_socket(line).unwrap();
        assert_eq!(socket.local, "192.168.0.2".parse::<IpAddr>().unwrap());
        assert_eq!((socket.pid, socket.received, socket.sent), (42, 250, 180));
        assert_eq!(socket.cookie, "a");
        assert!(parse_socket(&line.replace("fd=5))", "fd=5),(\"other\",pid=44,fd=6))")).is_none());
    }

    #[test]
    #[ignore = "requires iproute2 and a loopback socket"]
    fn attributes_live_tcp_bytes_to_the_process() {
        use std::{
            io::{Read, Write},
            net::{TcpListener, TcpStream},
            time::Duration,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiver, _) = listener.accept().unwrap();
        let mut sampler = Sampler::default();
        assert!(sampler.sample(true).is_empty());

        let bytes = [1; 4096];
        sender.write_all(&bytes).unwrap();
        let mut received = [0; 4096];
        receiver.read_exact(&mut received).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let rates = sampler.sample(true);
        let &(rx, tx) = &rates["lo"][&(std::process::id() as i32)];
        assert!(rx > 0.0 && tx > 0.0, "{rates:?}");
        assert!(sampler.sample(false).is_empty());
    }
}
