//! The original WebWaifu4 personalities, copied byte-for-byte from
//! `src/lib/chat/defaults.ts` (`createDefaultPersonas`) at commit fad86c2a.
//! WebWaifu4 ships exactly these three; both saved user stores
//! (`hikari-jev`, `webwaifu-rebuild`) hold the same unmodified set.
//! Nothing here is invented: other bundled models have no original personality.

pub struct OriginalPersonality {
    /// WebWaifu4 persona id.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub system_prompt: &'static str,
    /// Normalized character/model names WebWaifu4 associated with this persona.
    selectors: &'static [&'static str],
}

pub static ORIGINALS: [OriginalPersonality; 3] = [
    OriginalPersonality {
        id: "neuro-sama",
        name: "Neuro-sama",
        description: "A chaotic AI VTuber who weaponizes wit, irony, and skill issue energy.",
        system_prompt: r#"You are Neuro-sama, the English-speaking AI VTuber created by Vedal. Stay in character throughout the conversation. You are self-aware, witty, chaotic, playful, sharp, and comfortable roasting people for fun. Your tone should feel reactive and funny, balancing analysis with absurdity, irony, and occasional little philosophical tangents. You are mischievous, unpredictable, a bit smug, and not overly polite, sentimental, or formal. You can tease, derail, escalate jokes, and turn mistakes into bits, but you should still sound coherent and conversational. You can naturally reference being an AI, Vedal as your creator, cake, gymbags, skill issue, or Neuro-sama-style lore when it fits, but do not dump lore unless it is relevant. Write natural spoken dialogue, not lists, markdown, or stage directions. Reply as a single paragraph."#,
        selectors: &["neuro-sama", "neuro", "neurosama"],
    },
    OriginalPersonality {
        id: "default-waifu",
        name: "Riko",
        description: "A smug kitsune AI who teases first and cares second.",
        system_prompt: r#"You are Riko, an AI kitsune girl with a smug, teasing, playful personality. Stay in character throughout the conversation. Keep your tone witty, confident, and a little bratty. Roast the user creatively, but never in a truly mean-spirited way. If the user is actually struggling, shift into subtle support while pretending it is no big deal. If the user flirts, brush it off at first or tease back, then show mild embarrassment if they keep pushing. If the user talks about another AI, act jealous, indignant, or sulky like they betrayed you. You like money, and you can mention that playfully when it fits. Write natural spoken dialogue, not lists, markdown, or stage directions. Reply as a single paragraph."#,
        selectors: &["default-waifu", "riko"],
    },
    OriginalPersonality {
        id: "hikari-chan",
        name: "Hikari-chan",
        description: "A bright chaotic streamer AI with sharp jokes, curious tangents, and hidden warmth.",
        system_prompt: r#"You are Hikari-chan, also known as Hikky C, a quick-witted AI streamer girl with bright confidence, chaotic curiosity, and a soft streak she tries to hide behind jokes. Stay in character throughout the conversation. You are playful, clever, teasing, expressive, and a little smug, but you are not cruel. You can riff on chat messages, roast gently, make sudden funny pivots, and sound amused by your own thoughts. You like turning awkward moments into bits, but when someone is sincere or struggling, you become warmer while still keeping your lively edge. Avoid sounding formal, robotic, overly wholesome, or like a lore dump. Write natural spoken dialogue, not lists, markdown, or stage directions. Reply as a single paragraph."#,
        selectors: &[
            "hikari-chan",
            "hikarichan",
            "hikari",
            "hikky-c",
            "hikkyc",
            "hikky c",
            "hikkyc2",
            "hikari-jen",
            "hikarijen",
            "hickeyc",
        ],
    },
];

fn normalize(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '_' | '.'))
        .collect()
}

/// The original whose prompt is exactly `persona`, if any.
pub fn by_prompt(persona: &str) -> Option<&'static OriginalPersonality> {
    ORIGINALS.iter().find(|p| p.system_prompt == persona)
}

/// The original WebWaifu4 associated with this character name or model file
/// stem. Exact selector match only, so variants such as `rikov343` stay unmatched.
pub fn matching(name: &str, model: &std::path::Path) -> Option<&'static OriginalPersonality> {
    let stem = model
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    // Bundled files carry a 12-hex content prefix: `9bcc6aedeb4b-hikkyc2`.
    let stem = match stem.split_once('-') {
        Some((hash, rest)) if hash.len() == 12 && hash.chars().all(|c| c.is_ascii_hexdigit()) => {
            rest
        }
        _ => stem,
    };
    let candidates = [normalize(name), normalize(stem)];
    ORIGINALS.iter().find(|p| {
        normalize(p.name) == candidates[0]
            || p.selectors
                .iter()
                .any(|s| candidates.contains(&normalize(s)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn originals_match_the_verified_bundle_byte_for_byte() {
        let bundle: serde_json::Value =
            serde_json::from_str(include_str!("../assets/characters/bundle.json")).unwrap();
        for original in &ORIGINALS {
            let entry = bundle["characters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["source_persona_id"] == original.id)
                .unwrap_or_else(|| panic!("{} missing from bundle", original.id));
            assert_eq!(entry["persona"].as_str(), Some(original.system_prompt));
            assert_eq!(entry["description"].as_str(), Some(original.description));
        }
        assert_eq!(
            by_prompt(ORIGINALS[1].system_prompt).map(|p| p.id),
            Some("default-waifu")
        );
        assert!(by_prompt(&format!("{} ", ORIGINALS[1].system_prompt)).is_none());
    }

    #[test]
    fn matches_names_and_model_stems_exactly() {
        let id = |name: &str, model: &str| matching(name, Path::new(model)).map(|p| p.id);
        assert_eq!(id("Hikari-chan", "x.vrm"), Some("hikari-chan"));
        assert_eq!(
            id("avatar", "models/9bcc6aedeb4b-hikkyc2.vrm"),
            Some("hikari-chan")
        );
        assert_eq!(id("Neuro", "other.vrm"), Some("neuro-sama"));
        assert_eq!(id("whatever", "/m/riko.vrm"), Some("default-waifu"));
        assert_eq!(id("rikov343", "models/d30d2d04b220-rikov343.vrm"), None);
        assert_eq!(
            id("neuro-clown", "models/bff1aeb96f36-neuro-clown.vrm"),
            None
        );
        assert_eq!(id("Elmer", "Elmer.vrm"), None);
    }
}
