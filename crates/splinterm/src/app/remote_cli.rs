//! Non-network remote-profile inspection commands.

use std::{fmt::Write as _, time::Duration};

use anyhow::{Context, Result, bail};
use splinterm::{
    automation::Connection,
    remote::{RemoteCatalog, RemoteProfile},
    remote_session::RemoteSession,
};
use splinterm_protocol::{Request, Response};

use super::commands::RemoteCommand;

pub(in crate::app) async fn run_remote_command(command: RemoteCommand) -> Result<()> {
    let catalog = RemoteCatalog::load_default()?;
    match command {
        RemoteCommand::List => print!("{}", render_remote_list(&catalog)),
        RemoteCommand::Inspect { profile } => {
            print!("{}", render_remote_profile(catalog.get(&profile)?));
        }
        RemoteCommand::Check { profile } => {
            check_remote(catalog.get(&profile)?).await?;
        }
    }
    Ok(())
}

async fn check_remote(profile: &RemoteProfile) -> Result<()> {
    let session = RemoteSession::connect(profile)
        .await
        .with_context(|| format!("remote {} transport check failed", profile.name()))?;
    let mut connection: Connection = session
        .connect_automation()
        .await
        .with_context(|| format!("remote {} daemon handshake failed", profile.name()))?;
    let deadline = Duration::from_secs(u64::from(profile.connect_timeout_seconds()));
    if !matches!(
        connection
            .request_with_deadline(Request::Ping, deadline)
            .await?,
        Response::Pong
    ) {
        bail!("remote splinterd returned an invalid ping response");
    }
    let Response::Lairs { lairs, .. } = connection
        .request_with_deadline(Request::ListLairs, deadline)
        .await
        .context(
            "remote topology read failed; persistent policy may deny topology_metadata_read",
        )?
    else {
        bail!("remote splinterd returned an invalid topology response");
    };
    println!(
        "Remote {} is reachable ({} Lairs); this check does not enumerate all authority.",
        profile.name(),
        lairs.len()
    );
    Ok(())
}

fn render_remote_list(catalog: &RemoteCatalog) -> String {
    if catalog.is_empty() {
        return "No remote profiles configured.\n".to_owned();
    }
    let mut output = String::new();
    writeln!(output, "Remote profiles ({})", catalog.iter().len())
        .expect("writing to String cannot fail");
    for profile in catalog.iter() {
        write!(output, "  {:<20}  {}", profile.name(), profile.host())
            .expect("writing to String cannot fail");
        if let Some(user) = profile.user() {
            write!(output, "  user={user}").expect("writing to String cannot fail");
        }
        if let Some(port) = profile.port() {
            write!(output, "  port={port}").expect("writing to String cannot fail");
        }
        output.push('\n');
    }
    output
}

fn render_remote_profile(profile: &RemoteProfile) -> String {
    let mut output = String::new();
    writeln!(output, "Remote  {}", profile.name()).expect("writing to String cannot fail");
    writeln!(output, "  Host             {}", profile.host())
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "  User             {}",
        profile.user().unwrap_or("OpenSSH default")
    )
    .expect("writing to String cannot fail");
    if let Some(port) = profile.port() {
        writeln!(output, "  Port             {port}").expect("writing to String cannot fail");
    } else {
        writeln!(output, "  Port             OpenSSH default")
            .expect("writing to String cannot fail");
    }
    writeln!(
        output,
        "  Connect timeout  {} seconds",
        profile.connect_timeout_seconds()
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "  Identity files   {}",
        profile.identity_files().len()
    )
    .expect("writing to String cannot fail");
    for path in profile.identity_files() {
        writeln!(output, "    {}", path.display()).expect("writing to String cannot fail");
    }
    writeln!(
        output,
        "  Known hosts      {}",
        profile.known_hosts_file().map_or_else(
            || "OpenSSH default".to_owned(),
            |path| path.display().to_string()
        )
    )
    .expect("writing to String cannot fail");
    writeln!(output, "  SSH argv").expect("writing to String cannot fail");
    let plan = profile.ssh_plan();
    writeln!(output, "    {}", plan.program().to_string_lossy())
        .expect("writing to String cannot fail");
    for argument in plan.arguments() {
        writeln!(output, "    {:?}", argument.to_string_lossy())
            .expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> RemoteCatalog {
        RemoteCatalog::parse(
            r#"
version = 1
[remotes.wintermute]
host = "wintermute.example"
user = "operator"
port = 2222
"#,
            None,
        )
        .unwrap()
    }

    #[test]
    fn list_output_is_stable_and_calm() {
        let rendered = render_remote_list(&catalog());
        assert_eq!(
            rendered,
            "Remote profiles (1)\n  wintermute            wintermute.example  user=operator  port=2222\n"
        );
        assert_eq!(
            render_remote_list(&RemoteCatalog::default()),
            "No remote profiles configured.\n"
        );
    }

    #[test]
    fn inspect_output_shows_fixed_redacted_process_plan() {
        let catalog = catalog();
        let rendered = render_remote_profile(catalog.get("wintermute").unwrap());
        assert!(rendered.starts_with("Remote  wintermute\n"));
        assert!(rendered.contains("StrictHostKeyChecking=yes"));
        assert!(rendered.contains("/usr/bin/splinterm relay --graphical-stdio"));
        assert!(!rendered.to_ascii_lowercase().contains("password"));
        assert!(!rendered.to_ascii_lowercase().contains("private key"));
    }
}
