use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::UnixStream;

use getopts::Options;
use ini::Ini;
use nix::sys::utsname::uname;
use rpassword::prompt_password_stderr;

use greet_proto::{Header, QuestionStyle, Request, Response};

fn prompt_stderr(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdin_iter = stdin.lock().lines();
    eprint!("{}", prompt);
    Ok(stdin_iter.next().unwrap()?)
}

fn get_distro_name() -> String {
    Ini::load_from_file("/etc/os-release")
        .ok()
        .and_then(|file| {
            let section = file.general_section();
            Some(
                section
                    .get("PRETTY_NAME")
                    .unwrap_or(&"Linux".to_string())
                    .to_string(),
            )
        })
        .unwrap_or_else(|| "Linux".to_string())
}

fn get_issue() -> Result<String, Box<dyn std::error::Error>> {
    let vtnr: usize = env::var("XDG_VTNR")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .expect("unable to parse VTNR");
    let uts = uname();
    Ok(fs::read_to_string("/etc/issue")?
        .replace("\\S", &get_distro_name())
        .replace("\\l", &format!("tty{}", vtnr))
        .replace("\\s", &uts.sysname())
        .replace("\\r", &uts.release())
        .replace("\\v", &uts.version())
        .replace("\\n", &uts.nodename())
        .replace("\\m", &uts.machine())
        .replace("\\\\", "\\"))
}

fn login(node: &str, cmd: &Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let username = prompt_stderr(&format!("{} login: ", node))?;
    let command = match cmd {
        Some(cmd) => cmd.to_string(),
        None => prompt_stderr("Command: ")?,
    };

    let mut stream = UnixStream::connect(env::var("GREETD_SOCK")?)?;
    let mut request = Request::CreateSession { username };
    let mut starting = false;
    loop {
        let req = request.to_bytes()?;
        let header = Header::new(req.len() as u32);
        stream.write_all(&header.to_bytes()?)?;
        stream.write_all(&req)?;

        // Read response
        let mut header_buf = vec![0; Header::len()];
        stream.read_exact(&mut header_buf)?;
        let header = Header::from_slice(&header_buf)?;

        let mut resp_buf = vec![0; header.len as usize];
        stream.read_exact(&mut resp_buf)?;
        let resp = Response::from_slice(&resp_buf)?;

        match resp {
            Response::AuthQuestion { question, style } => {
                let answer = match style {
                    QuestionStyle::Visible => prompt_stderr(&question)?,
                    QuestionStyle::Secret => prompt_password_stderr(&question)?,
                    QuestionStyle::Info => {
                        eprintln!("info: {}", question);
                        "".to_string()
                    }
                    QuestionStyle::Error => {
                        eprintln!("error: {}", question);
                        "".to_string()
                    }
                };

                request = Request::AnswerAuthQuestion { answer: Some(answer) };
            },
            Response::Success => match starting {
                true => break,
                false => {
                    starting = true;
                    request = Request::StartSession {
                        env: vec![
                            format!("XDG_SESSION_DESKTOP={}", &command),
                            format!("XDG_CURRENT_DESKTOP={}", &command),
                        ],
                        cmd: vec![command.to_string()],
                    }
                }
            },
            Response::Error{ error_type: _, description } => return Err(format!("login error: {:?}", description).into()),
        }
    }
    Ok(())
}

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} [options]", program);
    print!("{}", opts.usage(&brief));
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();
    let mut opts = Options::new();
    opts.optflag("c", "cmd", "command to run");
    opts.optflag("f", "max-failures", "maximum number of accepted failures");
    opts.optflag("h", "help", "print this help menu");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            println!("{}", f.to_string());
            print_usage(&program, opts);
            std::process::exit(1);
        }
    };
    if matches.opt_present("h") {
        print_usage(&program, opts);
        std::process::exit(0);
    }

    let cmd = matches.opt_default("cmd", "");
    let max_failures: usize = match matches.opt_get("max-failures") {
        Ok(v) => v.unwrap_or(5),
        Err(e) => {
            eprintln!("unable to parse max failures: {}", e);
            std::process::exit(1)
        }
    };

    if let Ok(issue) = get_issue() {
        print!("{}", issue);
    }

    let uts = uname();
    for _ in 0..max_failures {
        match login(uts.nodename(), &cmd) {
            Ok(()) => {
                break;
            }
            Err(e) => eprintln!("error: {}", e),
        }
    }
}
