use serde::{Deserialize, Serialize};

/// Stable identity shared by settings, polling, themes, and context menus.
///
/// Adding a provider starts here: register its descriptor, implement its poller,
/// and connect any provider-specific settings persistence that older versions
/// need to understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ProviderId {
    Claude = 0,
    Codex = 1,
    Antigravity = 2,
    OpenCode = 3,
    Cursor = 4,
    Grok = 5,
    Fireworks = 6,
    Devin = 7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    /// Stable key used by theme expressions and context-menu documents.
    pub key: &'static str,
    /// Stable key used by the persisted usage cache.
    pub cache_key: &'static str,
    /// English catalogue key resolved through the localization layer.
    pub display_name: &'static str,
    /// English catalogue key for the provider setting description.
    pub settings_description: &'static str,
    /// Stable Win32 command id used by native provider menu items.
    pub native_menu_command_id: u16,
    /// Two letters the tray icon can carry so icons for different providers
    /// can be told apart at a glance.
    pub tray_mark: &'static str,
    /// Where to get the provider's tool, for the card of a provider that
    /// is not installed yet.
    pub install_url: &'static str,
    /// Every provider is on by default: a provider that is not installed
    /// says so on its own card, and switching one off is the user's call.
    pub default_enabled: bool,
}

pub const PROVIDER_DESCRIPTORS: [ProviderDescriptor; 8] = [
    ProviderDescriptor {
        id: ProviderId::Claude,
        key: "claude",
        cache_key: "claude_code",
        display_name: "Claude Code",
        settings_description: "Collect usage from Anthropic",
        native_menu_command_id: 60,
        tray_mark: "CL",
        install_url: "https://claude.com/claude-code",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Codex,
        key: "codex",
        cache_key: "codex",
        display_name: "Codex",
        settings_description: "Collect usage from OpenAI",
        native_menu_command_id: 61,
        tray_mark: "CX",
        install_url: "https://openai.com/codex",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Antigravity,
        key: "antigravity",
        cache_key: "antigravity",
        display_name: "Antigravity",
        settings_description: "Collect usage from Google",
        native_menu_command_id: 62,
        tray_mark: "AG",
        install_url: "https://antigravity.google",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::OpenCode,
        key: "opencode",
        cache_key: "opencode",
        display_name: "OpenCode",
        settings_description: "Collect usage from OpenCode Go",
        native_menu_command_id: 63,
        tray_mark: "OC",
        install_url: "https://opencode.ai",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Cursor,
        key: "cursor",
        cache_key: "cursor",
        display_name: "Cursor",
        settings_description: "Collect usage from Cursor",
        native_menu_command_id: 64,
        tray_mark: "CU",
        install_url: "https://cursor.com",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Grok,
        key: "grok",
        cache_key: "grok",
        display_name: "Grok",
        settings_description: "Collect usage from xAI",
        native_menu_command_id: 65,
        tray_mark: "GK",
        install_url: "https://grok.com",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Fireworks,
        key: "fireworks",
        cache_key: "fireworks",
        display_name: "Fireworks",
        settings_description: "Collect usage from Fireworks",
        native_menu_command_id: 66,
        tray_mark: "FW",
        install_url: "https://fireworks.ai",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Devin,
        key: "devin",
        cache_key: "devin",
        display_name: "Devin",
        settings_description: "Collect usage from Devin",
        native_menu_command_id: 67,
        tray_mark: "DV",
        install_url: "https://devin.ai",
        default_enabled: true,
    },
];

impl ProviderId {
    pub const ALL: [Self; 8] = [
        Self::Claude,
        Self::Codex,
        Self::Antigravity,
        Self::OpenCode,
        Self::Cursor,
        Self::Grok,
        Self::Fireworks,
        Self::Devin,
    ];

    pub const fn descriptor(self) -> &'static ProviderDescriptor {
        &PROVIDER_DESCRIPTORS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.key == key)
            .map(|descriptor| descriptor.id)
    }

    pub fn from_cache_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.cache_key == key)
            .map(|descriptor| descriptor.id)
    }

    pub fn from_native_menu_command_id(command_id: u16) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.native_menu_command_id == command_id)
            .map(|descriptor| descriptor.id)
    }
}

impl Default for ProviderId {
    fn default() -> Self {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.default_enabled)
            .map(|descriptor| descriptor.id)
            .unwrap_or(Self::Claude)
    }
}

/// Compact, copyable selection passed between settings, UI, and poll workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSet(u64);

impl ProviderSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_enabled(enabled: impl IntoIterator<Item = ProviderId>) -> Self {
        let mut providers = Self::empty();
        for provider in enabled {
            providers.set(provider, true);
        }
        providers
    }

    pub const fn contains(self, provider: ProviderId) -> bool {
        self.0 & provider.bit() != 0
    }

    pub fn set(&mut self, provider: ProviderId, enabled: bool) {
        if enabled {
            self.0 |= provider.bit();
        } else {
            self.0 &= !provider.bit();
        }
    }

    /// Toggle a provider while preserving the application invariant that at
    /// least one provider remains enabled.
    pub fn toggle(&mut self, provider: ProviderId) -> bool {
        let enabled = self.contains(provider);
        if enabled && self.len() == 1 {
            return false;
        }
        self.set(provider, !enabled);
        true
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn iter(self) -> impl Iterator<Item = ProviderId> {
        ProviderId::ALL
            .into_iter()
            .filter(move |provider| self.contains(*provider))
    }
}

impl Default for ProviderSet {
    fn default() -> Self {
        Self::from_enabled(
            PROVIDER_DESCRIPTORS
                .iter()
                .filter(|descriptor| descriptor.default_enabled)
                .map(|descriptor| descriptor.id),
        )
    }
}

impl ProviderId {
    const fn bit(self) -> u64 {
        1 << self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_set_comes_from_descriptors() {
        // Every provider ships on; one that is not installed says so on its
        // own card rather than being hidden behind a switch.
        assert_eq!(ProviderSet::default(), ProviderSet::from_enabled(ProviderId::ALL));
        assert!(PROVIDER_DESCRIPTORS.iter().all(|descriptor| descriptor.default_enabled));
    }

    #[test]
    fn provider_set_refuses_to_toggle_off_its_last_provider() {
        let mut providers = ProviderSet::from_enabled([ProviderId::Codex]);
        assert!(!providers.toggle(ProviderId::Codex));
        assert!(providers.contains(ProviderId::Codex));
    }

    #[test]
    fn provider_keys_round_trip_through_the_registry() {
        for descriptor in PROVIDER_DESCRIPTORS {
            assert_eq!(ProviderId::from_key(descriptor.key), Some(descriptor.id));
            assert_eq!(
                ProviderId::from_cache_key(descriptor.cache_key),
                Some(descriptor.id)
            );
            assert_eq!(
                ProviderId::from_native_menu_command_id(descriptor.native_menu_command_id),
                Some(descriptor.id)
            );
        }
    }
}
