//! Network throughput from the system's cumulative byte counters.

use std::time::{Duration, Instant};

/// A rate as the keyboard shows it: `640 KB/s`, `2.0 MB/s`.
pub fn format_rate(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1024 * 1024 {
        format!("{:.1} MB/s", (bytes_per_sec as f64) / (1024.0 * 1024.0))
    } else {
        format!("{} KB/s", bytes_per_sec / 1024)
    }
}

/// The shortest time a rate is measured over, so frequent readings don't make it jumpy.
const MIN_WINDOW: Duration = Duration::from_secs(1);

/// Download and upload rates in bytes per second, from successive readings of the total bytes
/// received and sent.
#[derive(Debug, Default)]
pub struct RateMeter {
    prev: Option<(Instant, u64, u64)>,
}

impl RateMeter {
    /// The rates since the previous measured reading, once at least a second has passed; `None`
    /// before that. Totals that went backwards (an interface went away) start a new baseline.
    pub fn update(&mut self, now: Instant, rx_total: u64, tx_total: u64) -> Option<(u64, u64)> {
        if let Some((then, rx0, tx0)) = self.prev {
            let fresh_baseline = rx_total < rx0 || tx_total < tx0;
            if !fresh_baseline && now.saturating_duration_since(then) < MIN_WINDOW {
                return None;
            }
        }
        let prev = self.prev.replace((now, rx_total, tx_total));
        let (then, rx0, tx0) = prev?;
        let secs = now.saturating_duration_since(then).as_secs_f64();
        if secs <= 0.0 || rx_total < rx0 || tx_total < tx0 {
            return None;
        }
        Some((
            ((rx_total - rx0) as f64 / secs) as u64,
            ((tx_total - tx0) as f64 / secs) as u64,
        ))
    }
}

/// One interface's counters from `/proc/net/dev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceBytes {
    pub name: String,
    pub rx: u64,
    pub tx: u64,
}

/// Parses Linux `/proc/net/dev`.
pub fn parse_proc_net_dev(text: &str) -> Vec<InterfaceBytes> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let fields: Vec<u64> = rest
                .split_whitespace()
                .map(|f| f.parse::<u64>())
                .collect::<Result<_, _>>()
                .ok()?;
            Some(InterfaceBytes {
                name: name.trim().to_string(),
                rx: *fields.first()?,
                tx: *fields.get(8)?,
            })
        })
        .collect()
}

/// The total bytes received and sent over the interfaces that reach the network. Traffic through
/// bridges, container links and tunnels also crosses a physical interface, so only the interfaces
/// `is_physical` accepts are counted; when it accepts none, every interface but loopback is.
pub fn sum_interfaces(ifaces: &[InterfaceBytes], is_physical: impl Fn(&str) -> bool) -> (u64, u64) {
    let physical: Vec<&InterfaceBytes> = ifaces.iter().filter(|i| is_physical(&i.name)).collect();
    let counted: Vec<&InterfaceBytes> = if physical.is_empty() {
        ifaces.iter().filter(|i| i.name != "lo").collect()
    } else {
        physical
    };
    counted.iter().fold((0, 0), |(rx, tx), i| {
        (rx.saturating_add(i.rx), tx.saturating_add(i.tx))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_come_from_counter_deltas_and_survive_a_reset() {
        let t0 = Instant::now();
        let mut m = RateMeter::default();
        assert_eq!(m.update(t0, 1_000, 500), None);
        // Readings closer together than a second wait for a longer window.
        assert_eq!(m.update(t0 + Duration::from_millis(250), 9_000, 900), None);
        assert_eq!(
            m.update(t0 + Duration::from_secs(2), 2_049_000, 66_036),
            Some((1_024_000, 32_768))
        );
        // A counter that went backwards starts again rather than reporting a huge rate.
        assert_eq!(m.update(t0 + Duration::from_secs(3), 10, 10), None);
        assert_eq!(
            m.update(t0 + Duration::from_secs(4), 1_034, 10),
            Some((1_024, 0))
        );
        assert_eq!(format_rate(2 * 1024 * 1024), "2.0 MB/s");
        assert_eq!(format_rate(65_536), "64 KB/s");
        assert_eq!(format_rate(0), "0 KB/s");
    }

    #[test]
    fn proc_net_dev_counts_physical_interfaces_only() {
        let text = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 9000      10    0    0    0     0          0         0     9000      10    0    0    0     0       0          0
  eth0: 123456789 1000  0    0    0     0          0         0 987654   900    0    0    0     0       0          0
docker0: 5000     10    0    0    0     0          0         0     6000      10    0    0    0     0       0          0
 wlan0:    100     1    0    0    0     0          0         0      200       1    0    0    0     0       0          0
";
        let ifaces = parse_proc_net_dev(text);
        assert_eq!(ifaces.len(), 4);
        assert_eq!(
            ifaces[1],
            InterfaceBytes {
                name: "eth0".into(),
                rx: 123_456_789,
                tx: 987_654
            }
        );
        assert_eq!(
            sum_interfaces(&ifaces, |n| n == "eth0" || n == "wlan0"),
            (123_456_889, 987_854)
        );
        // Without physical interfaces, everything but loopback counts.
        assert_eq!(
            sum_interfaces(&ifaces, |_| false),
            (123_456_789 + 5_000 + 100, 987_654 + 6_000 + 200)
        );
    }
}
