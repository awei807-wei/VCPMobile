use serde::{Deserialize, Serialize};
use std::fmt;

/// Manifest 状态集合类别。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ManifestType {
    Owner,
    Topic,
    Avatar,
}

impl fmt::Display for ManifestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestType::Owner => write!(f, "owner"),
            ManifestType::Topic => write!(f, "topic"),
            ManifestType::Avatar => write!(f, "avatar"),
        }
    }
}

/// Agent 与 Group 的业务命名空间。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum OwnerType {
    Agent,
    Group,
}

impl OwnerType {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerType::Agent => "agent",
            OwnerType::Group => "group",
        }
    }
}

impl fmt::Display for OwnerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for OwnerType {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "agent" => Ok(OwnerType::Agent),
            "group" => Ok(OwnerType::Group),
            _ => Err(()),
        }
    }
}

/// Avatar 允许的命名空间；`user` 只允许固定的 `user_avatar` 身份。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AvatarOwnerType {
    Agent,
    Group,
    User,
}

impl AvatarOwnerType {
    pub fn as_str(self) -> &'static str {
        match self {
            AvatarOwnerType::Agent => "agent",
            AvatarOwnerType::Group => "group",
            AvatarOwnerType::User => "user",
        }
    }
}

impl fmt::Display for AvatarOwnerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for AvatarOwnerType {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "agent" => Ok(AvatarOwnerType::Agent),
            "group" => Ok(AvatarOwnerType::Group),
            "user" => Ok(AvatarOwnerType::User),
            _ => Err(()),
        }
    }
}

/// Protocol-level avatar identity contract. Agent and group avatars use a
/// non-empty entity id; the only user avatar identity is the fixed singleton.
pub fn is_valid_avatar_owner(owner_type: &str, owner_id: &str) -> bool {
    match AvatarOwnerType::try_from(owner_type) {
        Ok(AvatarOwnerType::Agent | AvatarOwnerType::Group) => !owner_id.is_empty(),
        Ok(AvatarOwnerType::User) => owner_id == "user_avatar",
        Err(()) => false,
    }
}
