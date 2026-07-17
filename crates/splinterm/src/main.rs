use std::{env, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use splinterm_protocol::{Envelope, Request, Response};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

#[derive(Debug, Parser)]
#[command(version, about = "Splinterm terminal client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check whether splinterd is reachable.
    Ping,
    /// List persistent dojos.
    List,
    /// Create a dojo with one shell splint.
    New {
        name: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let request = match Cli::parse().command {
        Command::Ping => Request::Ping,
        Command::List => Request::ListDojos,
        Command::New { name, cwd } => Request::CreateDojo {
            name,
            cwd: cwd.unwrap_or(env::current_dir().context("failed to read current directory")?),
        },
    };

    let response = exchange(request).await?;
    print_response(response)
}

async fn exchange(request: Request) -> Result<Response> {
    let socket = socket_path()?;
    let mut stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("cannot connect to splinterd at {}", socket.display()))?;

    let mut encoded = serde_json::to_vec(&Envelope::new(request))?;
    encoded.push(b'\n');
    stream.write_all(&encoded).await?;

    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).await?;
    if response.is_empty() {
        bail!("splinterd closed the connection without replying");
    }

    let envelope: Envelope<Response> =
        serde_json::from_str(&response).context("splinterd sent an invalid response")?;
    Ok(envelope.message)
}

fn print_response(response: Response) -> Result<()> {
    match response {
        Response::Pong => println!("splinterd is awake"),
        Response::Dojos { dojos } if dojos.is_empty() => println!("No dojos in the lair."),
        Response::Dojos { dojos } => {
            for dojo in dojos {
                let splints: usize = dojo
                    .windows
                    .iter()
                    .map(|window| window.root.splint_count())
                    .sum();
                println!(
                    "{}  {} window(s)  {splints} splint(s)",
                    dojo.name,
                    dojo.windows.len()
                );
            }
        }
        Response::DojoCreated { dojo } => println!("Created dojo '{}'.", dojo.name),
        Response::Error { message } => bail!("splinterd: {message}"),
    }
    Ok(())
}

fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }

    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}
