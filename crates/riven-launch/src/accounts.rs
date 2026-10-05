use std::path::PathBuf;

use md5::{Digest, Md5};
use riven_format::{Account, AccountKind, Accounts};

use crate::LaunchError;

pub fn path() -> Result<PathBuf, LaunchError> {
    crate::config_file("accounts.json")
}

pub fn load() -> Result<Accounts, LaunchError> {
    crate::load_or_default(&path()?)
}

pub fn save(accounts: &Accounts) -> Result<(), LaunchError> {
    crate::save(&path()?, accounts)
}

/// The UUID the vanilla server gives an offline player: `nameUUIDFromBytes("OfflinePlayer:" + name)`.
pub fn offline_uuid(name: &str) -> String {
    let mut bytes: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// An offline account; the name must be a valid Minecraft player name.
pub fn offline(name: &str) -> Result<Account, LaunchError> {
    let valid = (3..=16).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(LaunchError::BadName(name.to_owned()));
    }
    Ok(Account {
        id: offline_uuid(name),
        name: name.to_owned(),
        kind: AccountKind::Offline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuids_match_the_vanilla_server() {
        assert_eq!(
            offline_uuid("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert!(offline("Steve_1").is_ok());
        assert!(offline("ab").is_err());
        assert!(offline("Игрок").is_err());
    }
}
