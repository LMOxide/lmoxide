//! Administration subcommands that work on the database directly (also while the server
//! runs): accounts and the login key.

use std::io::IsTerminal;

use anyhow::{Context, bail};
use lmoxide_db::{Ban, Db};
use lmoxide_net::login::LoginKey;

use crate::Storage;

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// Manage accounts.
    #[command(subcommand)]
    Account(AccountCmd),
    /// Print the login public key clients are patched with (created on first use).
    LoginKey {
        /// Make a new key pair. Every client then needs patching again.
        #[arg(long)]
        regenerate: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum AccountCmd {
    /// Create an account. The password is asked for (or read from stdin).
    Create {
        username: String,
        #[arg(long, default_value_t = 0)]
        gm_level: u32,
    },
    /// Set an account's password (asked for, or read from stdin).
    Passwd { username: String },
    /// Set an account's GM level (0 = player).
    Gm { username: String, level: u32 },
    /// Ban an account (and end its sessions).
    Ban {
        username: String,
        /// Ban length in days; permanent without it.
        #[arg(long)]
        days: Option<u32>,
        #[arg(long)]
        reason: Option<String>,
    },
    Unban { username: String },
    /// List accounts.
    List {
        /// Only names containing this.
        search: Option<String>,
    },
    /// Delete an account with its character and friendships.
    Delete {
        username: String,
        /// Don't ask.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(cmd: Cmd, storage: &Storage) -> anyhow::Result<()> {
    let db = storage.open()?;
    match cmd {
        Cmd::Account(cmd) => account(&db, cmd),
        Cmd::LoginKey { regenerate } => {
            let key = login_key(&db, regenerate)?;
            println!("{}", key.public_hex());
            Ok(())
        }
    }
}

fn account(db: &Db, cmd: AccountCmd) -> anyhow::Result<()> {
    let find = |name: &str| db.account_by_name(name)?.with_context(|| format!("no account {name:?}"));
    match cmd {
        AccountCmd::Create { username, gm_level } => {
            let password = read_password()?;
            let a = db.create_account(&username, Some(&password), gm_level)?;
            db.audit_account("cli", "account-create", &a, None)?;
            println!("created {} (id {})", a.username, a.id);
        }
        AccountCmd::Passwd { username } => {
            let a = find(&username)?;
            let password = read_password()?;
            db.set_password(a.id, Some(&password))?;
            db.end_game_sessions(a.id)?;
            db.audit_account("cli", "account-passwd", &a, None)?;
            println!("password of {} set", a.username);
        }
        AccountCmd::Gm { username, level } => {
            let a = find(&username)?;
            db.set_gm_level(a.id, level)?;
            db.audit_account("cli", "account-gm", &a, Some(&level.to_string()))?;
            println!("{} now has GM level {level} (from their next login)", a.username);
        }
        AccountCmd::Ban { username, days, reason } => {
            let a = find(&username)?;
            let until = days.map(|d| lmoxide_db::now() + i64::from(d) * 86400);
            db.ban(a.id, &Ban { until, reason: reason.clone() })?;
            db.end_game_sessions(a.id)?;
            db.audit_account("cli", "account-ban", &a, reason.as_deref())?;
            println!("{} banned {}", a.username, days.map_or("permanently".into(), |d| format!("for {d} days")));
            println!("(a running server keeps an online player connected until kicked through lmoxide-api)");
        }
        AccountCmd::Unban { username } => {
            let a = find(&username)?;
            db.unban(a.id)?;
            db.audit_account("cli", "account-unban", &a, None)?;
            println!("{} unbanned", a.username);
        }
        AccountCmd::List { search } => {
            println!("{:<10} {:<24} {:>3} {:<8} ban", "id", "username", "gm", "password");
            for a in db.list_accounts(search.as_deref(), 0, u32::MAX)? {
                let ban = a.active_ban().map_or(String::new(), |b| match b.until {
                    Some(t) => format!("until {t} (unix)"),
                    None => "permanent".into(),
                });
                println!("{:<10} {:<24} {:>3} {:<8} {}", a.id, a.username, a.gm_level, if a.has_password { "set" } else { "-" }, ban);
            }
        }
        AccountCmd::Delete { username, yes } => {
            let a = find(&username)?;
            if !yes {
                if !std::io::stdin().is_terminal() {
                    bail!("pass --yes to delete without a terminal");
                }
                eprint!("delete {} and its character for good? [y/N] ", a.username);
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("y") {
                    bail!("not deleted");
                }
            }
            db.delete_account(a.id)?;
            db.audit_account("cli", "account-delete", &a, None)?;
            println!("deleted {}", a.username);
        }
    }
    Ok(())
}

/// Asks twice on a terminal; otherwise reads one line from stdin (scripts).
fn read_password() -> anyhow::Result<String> {
    if std::io::stdin().is_terminal() {
        let p = rpassword::prompt_password("password: ")?;
        if rpassword::prompt_password("again: ")? != p {
            bail!("the passwords differ");
        }
        Ok(p)
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

const LOGIN_KEY_SETTING: &str = "login_private_key";

/// The server's login key, created and stored in the database on first use.
/// Gives `db` the login key of `from` (creating one there if it has none).
pub fn share_login_key(from: &Db, db: &Db) -> anyhow::Result<()> {
    let key = login_key(from, false)?;
    if db.setting(LOGIN_KEY_SETTING)?.as_deref() != Some(key.private_hex().as_str()) {
        db.set_setting(LOGIN_KEY_SETTING, &key.private_hex())?;
        db.audit("cli", "login-key-share", None, None)?;
        tracing::info!("login key copied from the shared database");
    }
    Ok(())
}

pub fn login_key(db: &Db, regenerate: bool) -> anyhow::Result<LoginKey> {
    if !regenerate && let Some(hex) = db.setting(LOGIN_KEY_SETTING)? {
        return LoginKey::from_private_hex(&hex).map_err(|e| anyhow::anyhow!("stored login key: {e}"));
    }
    let key = LoginKey::generate();
    db.set_setting(LOGIN_KEY_SETTING, &key.private_hex())?;
    db.audit("cli", if regenerate { "login-key-regenerate" } else { "login-key-create" }, None, None)?;
    tracing::info!("new login key created");
    Ok(key)
}
