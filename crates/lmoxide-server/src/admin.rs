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
    /// Copy the characters of another server's database into this one, in game mode `--mode`
    /// (e.g. the old world editor server's var/survey-data/lmoxide.db as world-editor
    /// characters). Accounts are matched by username (made here if missing, with their
    /// password); characters get new ids; one already copied (same account, name and mode) is
    /// left alone.
    ImportCharacters {
        /// The other database file.
        from: std::path::PathBuf,
        #[arg(long, default_value = "world-editor")]
        mode: String,
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
    /// Change an account's username (its login name).
    Rename { username: String, new_username: String },
    /// Rename a character (ids: `account list-characters`). For a character in play use
    /// `!rename` or the API: its server would save the old name back.
    RenameCharacter {
        character: u32,
        /// The new name (1-3 words; quote it).
        name: String,
    },
    /// List an account's characters.
    ListCharacters { username: String },
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
        Cmd::ImportCharacters { from, mode } => import_characters(&db, &from, &mode),
    }
}

fn import_characters(db: &Db, from: &std::path::Path, mode: &str) -> anyhow::Result<()> {
    anyhow::ensure!(crate::instance::Mode::parse(mode).is_some(), "{mode}: one of progression, maxed, world-editor");
    let other = Db::open(from).with_context(|| format!("opening {}", from.display()))?;
    share_accounts(&other, db, None)?;
    let (mut copied, mut kept) = (0, 0);
    let mut offset = 0;
    loop {
        let page = other.list_accounts(None, offset, 500)?;
        offset += page.len() as u32;
        for theirs in &page {
            let Some(ours) = db.account_by_name(&theirs.username)? else { continue };
            let here = db.characters_of(ours.id)?;
            for summary in other.characters_of(theirs.id)? {
                let Some(mut c) = other.load_character(summary.id)? else { continue };
                if here.iter().any(|h| h.name == c.name && h.mode == mode) {
                    kept += 1;
                    continue;
                }
                c.mode = mode.to_string();
                let id = db.new_character_id()?;
                match db.save_character(id, ours.id, &c) {
                    Err(lmoxide_db::Error::NameTaken) => {
                        println!("{} ({}): {} skipped: another account has a character with this name", ours.username, ours.id, c.name);
                        continue;
                    }
                    r => r?,
                }
                println!("{} ({}): {} -> character {id}, {mode}", ours.username, ours.id, c.name);
                copied += 1;
            }
        }
        if page.len() < 500 {
            break;
        }
    }
    db.audit("cli", "import-characters", None, Some(&format!("{} ({mode}): {copied} copied", from.display())))?;
    println!("{copied} characters copied, {kept} were here already");
    Ok(())
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
        AccountCmd::Rename { username, new_username } => {
            let a = find(&username)?;
            db.rename_account(a.id, &new_username)?;
            db.audit_account("cli", "account-rename", &a, Some(&new_username))?;
            println!("{} is now {new_username}", a.username);
        }
        AccountCmd::RenameCharacter { character, name } => {
            let old = db.character_name(character)?.with_context(|| format!("no character {character}"))?;
            let name = db.rename_character(character, &name)?;
            db.audit("cli", "character-rename", Some(&character.to_string()), Some(&format!("{old} -> {name}")))?;
            println!("character {character}: {old} -> {name}");
            println!("(a running server saves the old name back while the character plays: use !rename or the API then)");
        }
        AccountCmd::ListCharacters { username } => {
            let a = find(&username)?;
            println!("{:<10} {:<14} {:<8} name", "id", "mode", "playfield");
            for c in db.characters_of(a.id)? {
                println!("{:<10} {:<14} {:<8} {}", c.id, c.mode, c.playfield, if c.name.is_empty() { "(on the creation screen)" } else { &c.name });
            }
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
/// Copies the logins (usernames and password hashes) of `from` into `db`; `only`: just that
/// username. GM levels, bans and characters stay `db`'s own. How many changed.
pub fn share_accounts(from: &Db, db: &Db, only: Option<&str>) -> anyhow::Result<usize> {
    let accounts = match only {
        Some(name) => from.account_by_name(name)?.into_iter().collect(),
        None => {
            let mut all = Vec::new();
            loop {
                let page = from.list_accounts(None, all.len() as u32, 500)?;
                let done = page.len() < 500;
                all.extend(page);
                if done {
                    break all;
                }
            }
        }
    };
    let mut changed = 0;
    for a in accounts {
        let hash = from.password_hash(a.id)?;
        match db.import_login(a.id, &a.username, hash.as_deref()) {
            Ok(true) => changed += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(username = %a.username, "shared account not copied: {e}"),
        }
    }
    Ok(changed)
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
