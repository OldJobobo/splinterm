#![forbid(unsafe_code)]

use std::{env, process, thread, time::Duration};

fn main() {
    let mut arguments = env::args().skip(1);
    let role = arguments.next().expect("role argument");
    let bytes: usize = arguments
        .next()
        .expect("byte argument")
        .parse()
        .expect("bytes must be an integer");
    let seconds: u64 = arguments
        .next()
        .unwrap_or_else(|| "10".to_owned())
        .parse()
        .expect("seconds must be an integer");
    assert!(arguments.next().is_none(), "unexpected argument");

    let mut allocation = vec![0_u8; bytes];
    for offset in (0..bytes).step_by(4096) {
        allocation[offset] = u8::try_from(offset / 4096 % 251).unwrap();
    }
    let checksum = allocation
        .iter()
        .fold(0_u64, |sum, value| sum + u64::from(*value));
    println!(
        "{{\"role\":\"{role}\",\"pid\":{},\"bytes\":{bytes},\"checksum\":{checksum}}}",
        process::id()
    );
    thread::sleep(Duration::from_secs(seconds));
    std::hint::black_box(allocation);
}
