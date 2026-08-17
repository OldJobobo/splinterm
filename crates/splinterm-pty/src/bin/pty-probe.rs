use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    process,
};

fn main() {
    let mode = env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "inspect" => inspect(),
        "resize" => resize(),
        "echo" => echo(),
        "handoff" => handoff(),
        "argv" => argv(),
        "wait" => wait(),
        _ => process::exit(64),
    }
}

fn inspect() {
    let pid = rustix::process::getpid();
    println!("PID={}", pid.as_raw_nonzero());
    println!(
        "SID={}",
        rustix::process::getsid(None).unwrap().as_raw_nonzero()
    );
    println!("PGRP={}", rustix::process::getpgrp().as_raw_nonzero());
    println!(
        "TTY_SID={}",
        rustix::termios::tcgetsid(io::stdin())
            .unwrap()
            .as_raw_nonzero()
    );
    println!(
        "TTY_PGRP={}",
        rustix::termios::tcgetpgrp(io::stdin())
            .unwrap()
            .as_raw_nonzero()
    );
    let termios = rustix::termios::tcgetattr(io::stdin()).unwrap();
    println!(
        "IUTF8={}",
        u8::from(
            termios
                .input_modes
                .contains(rustix::termios::InputModes::IUTF8)
        )
    );
    println!(
        "TTY={}{}{}",
        u8::from(io::stdin().is_terminal()),
        u8::from(io::stdout().is_terminal()),
        u8::from(io::stderr().is_terminal())
    );
    println!("CWD={}", env::current_dir().unwrap().display());
    println!("TERM={}", env::var("TERM").unwrap_or_default());
    println!("COLORTERM={}", env::var("COLORTERM").unwrap_or_default());
    println!(
        "CUSTOM={}",
        env::var("SPLINTERM_PTY_TEST").unwrap_or_default()
    );
    println!("FOREIGN={}", env::var("TERM_PROGRAM").unwrap_or_default());
    let mut descriptors = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            std::fs::read_link(entry.path())
                .ok()
                .map(|target| format!("{}:{}", name.to_string_lossy(), target.display()))
        })
        .collect::<Vec<_>>();
    descriptors.sort();
    println!("FDS={}", descriptors.join(","));
}

fn resize() {
    let initial = rustix::termios::tcgetwinsize(io::stdin()).unwrap();
    println!(
        "INITIAL={}x{}+{}x{}",
        initial.ws_col, initial.ws_row, initial.ws_xpixel, initial.ws_ypixel
    );
    println!("READY");
    io::stdout().flush().unwrap();
    io::stdin().lock().lines().next().unwrap().unwrap();
    let resized = rustix::termios::tcgetwinsize(io::stdin()).unwrap();
    println!(
        "RESIZED={}x{}+{}x{}",
        resized.ws_col, resized.ws_row, resized.ws_xpixel, resized.ws_ypixel
    );
}

fn echo() {
    println!("READY");
    io::stdout().flush().unwrap();
    let line = io::stdin().lock().lines().next().unwrap().unwrap();
    println!("ECHO:{line}");
}

fn handoff() {
    println!("READY");
    io::stdout().flush().unwrap();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        match line.as_str() {
            "exit" => break,
            "size" => {
                let size = rustix::termios::tcgetwinsize(io::stdin()).unwrap();
                println!(
                    "SIZE={}x{}+{}x{}",
                    size.ws_col, size.ws_row, size.ws_xpixel, size.ws_ypixel
                );
            }
            _ if line.starts_with("burst ") => {
                let mut fields = line.split_whitespace();
                assert_eq!(fields.next(), Some("burst"));
                let label = fields.next().unwrap();
                let count = fields.next().unwrap().parse::<usize>().unwrap();
                assert!(fields.next().is_none());
                for index in 0..count {
                    println!("BURST:{label}:{index:04}");
                }
            }
            _ => println!("ECHO:{line}"),
        }
        io::stdout().flush().unwrap();
    }
}

fn argv() {
    println!("ARGV0={}", env::args().next().unwrap());
}

fn wait() {
    println!("READY");
    io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
