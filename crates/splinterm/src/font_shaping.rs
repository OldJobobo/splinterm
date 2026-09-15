//! Validated startup-only terminal shaping policy; independent of font source identity.
use anyhow::{Result, ensure};
use swash::Setting;

const MAX_FEATURES: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontLigatures {
    #[default]
    Off,
    On,
    Cursor,
}

/// Exact four-byte printable ASCII tags, case preserved, sorted with no duplicates.
/// Values are unsigned OpenType feature selectors, not just boolean switches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeatureSettings(Vec<Setting<u16>>);

impl FeatureSettings {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        if value.is_empty() {
            return Ok(Self::default());
        }
        let settings = value
            .split(',')
            .take(MAX_FEATURES + 1)
            .map(|item| {
                let (tag, value) = item
                    .trim()
                    .rsplit_once('=')
                    .ok_or_else(|| anyhow::anyhow!("expected tag=value feature setting"))?;
                ensure!(
                    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
                    "feature selector must be an unsigned u16"
                );
                Ok((tag, value.parse::<i64>()?))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(&settings)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Setting<u16>> + '_ {
        self.0.iter().copied()
    }

    pub(crate) fn new(settings: &[(&str, i64)]) -> Result<Self> {
        ensure!(settings.len() <= MAX_FEATURES, "too many feature settings");
        let mut normalized = Vec::with_capacity(settings.len());
        for &(tag, value) in settings {
            ensure!(
                tag.len() == 4 && tag.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
                "feature tag must be exactly four printable ASCII bytes"
            );
            let value = u16::try_from(value)?;
            normalized.push(Setting {
                tag: u32::from_be_bytes(tag.as_bytes().try_into()?),
                value,
            });
        }
        normalized.sort_unstable_by_key(|setting| setting.tag);
        ensure!(
            normalized.windows(2).all(|pair| pair[0].tag != pair[1].tag),
            "duplicate feature tag"
        );
        Ok(Self(normalized))
    }
}
