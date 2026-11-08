//! The system's own speech off macOS (macOS uses `say`, in voice.rs):
//!
//! - Windows: the speech engine every Windows has (System.Speech, through
//!   PowerShell), to a WAV file. Its voices say whether they're a woman's
//!   or a man's.
//! - Linux: `espeak-ng` (or the older `espeak`), to a WAV file, with its
//!   female / male variants of the English voice. Robotic, but offline.
//!
//! The text goes in on stdin and paths through the environment, so nothing
//! the agent writes is ever parsed as a command.

use std::path::Path;
#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
use anyhow::Context as _;
use anyhow::{Result, bail};

/// A Windows voice: its name, culture ("en-US") and whether it's a woman's.
#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    pub name: String,
    pub culture: String,
    pub female: bool,
}

/// `Name|Culture|Gender` lines (what `LIST` prints) → voices.
#[cfg(any(windows, test))]
pub fn parse_voices(text: &str) -> Vec<Voice> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim().split('|');
            let name = parts.next()?.trim();
            let culture = parts.next()?.trim();
            let gender = parts.next()?.trim();
            (!name.is_empty()).then(|| Voice {
                name: name.to_string(),
                culture: culture.to_string(),
                female: gender.eq_ignore_ascii_case("female"),
            })
        })
        .collect()
}

/// The voice for `wanted` (a name, "female", "male"): one asked for by
/// name, else an English one of the wanted kind (a woman's by default),
/// US English first.
#[cfg(any(windows, test))]
pub fn pick(voices: &[Voice], wanted: Option<&str>) -> Option<String> {
    let w = wanted.map(|w| w.trim().to_lowercase()).unwrap_or_default();
    if !w.is_empty()
        && let Some(v) = voices.iter().find(|v| {
            let name = v.name.to_lowercase();
            name == w || name.split_whitespace().any(|word| word == w)
        })
    {
        return Some(v.name.clone());
    }
    let male = matches!(w.as_str(), "male" | "man" | "boy" | "m");
    let rank = |v: &Voice| {
        (
            !v.culture.to_lowercase().starts_with("en"),
            v.female == male,
            !v.culture.eq_ignore_ascii_case("en-US"),
        )
    };
    voices
        .iter()
        .filter(|v| v.culture.to_lowercase().starts_with("en"))
        .min_by_key(|v| rank(v))
        .map(|v| v.name.clone())
}

/// Lists the voices (Windows).
#[cfg(windows)]
const LIST: &str = "Add-Type -AssemblyName System.Speech; \
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
    foreach ($v in $s.GetInstalledVoices()) { if ($v.Enabled) { \
    $i = $v.VoiceInfo; Write-Output ($i.Name + '|' + $i.Culture.Name + '|' + $i.Gender) } }";

/// Speaks stdin in voice $env:REEL_VOICE to the WAV $env:REEL_OUT (Windows).
#[cfg(windows)]
const SPEAK: &str = "Add-Type -AssemblyName System.Speech; \
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
    $s.SelectVoice($env:REEL_VOICE); $s.Rate = 1; \
    $s.SetOutputToWaveFile($env:REEL_OUT); \
    $s.Speak([Console]::In.ReadToEnd()); $s.Dispose()";

#[cfg(windows)]
fn powershell() -> Command {
    use std::os::windows::process::CommandExt as _;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command"]);
    // No console window flashing up.
    cmd.creation_flags(0x0800_0000);
    cmd
}

/// The voice to use for `wanted` on this system.
pub fn system_voice(wanted: Option<&str>) -> Result<String> {
    #[cfg(windows)]
    {
        let out = powershell()
            .arg(LIST)
            .output()
            .context("couldn't ask Windows for its voices")?;
        let voices = parse_voices(&String::from_utf8_lossy(&out.stdout));
        pick(&voices, wanted).ok_or_else(|| {
            anyhow::anyhow!("No system voice is installed (Settings › Time & language › Speech)")
        })
    }
    #[cfg(not(windows))]
    {
        let Some(program) = espeak() else {
            bail!(
                "No voice: install espeak-ng (the system speech), or save an OpenAI or Gemini key in the agent's setup."
            );
        };
        let male = matches!(
            wanted.map(|w| w.trim().to_lowercase()).as_deref(),
            Some("male" | "man" | "boy" | "m")
        );
        Ok(format!(
            "{program} en-us+{}",
            if male { "m3" } else { "f3" }
        ))
    }
}

/// `espeak-ng`, else `espeak`, if on the PATH.
#[cfg(not(windows))]
fn espeak() -> Option<&'static str> {
    ["espeak-ng", "espeak"].into_iter().find(|p| {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(p).is_file()))
    })
}

/// Says `text` in `voice` (from `system_voice`) into the WAV at `path`.
pub fn speak(text: &str, voice: &str, path: &Path) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("part.wav");
    #[cfg(windows)]
    let mut child = powershell()
        .arg(SPEAK)
        .env("REEL_VOICE", voice)
        .env("REEL_OUT", &tmp)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .context("couldn't run the system speech")?;
    #[cfg(not(windows))]
    let mut child = {
        let (program, variant) = voice.split_once(' ').unwrap_or((voice, "en-us"));
        std::process::Command::new(program)
            .args(["-v", variant, "-s", "175", "-w"])
            .arg(&tmp)
            .arg("--stdin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("couldn't run {program}: {e}"))?
    };
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() || !tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
        bail!("The system voice \"{voice}\" couldn't speak this");
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_picks_windows_voices() {
        let voices = parse_voices(
            "Microsoft David Desktop|en-US|Male\r\nMicrosoft Hazel Desktop|en-GB|Female\r\n\
             Microsoft Zira Desktop|en-US|Female\r\nMicrosoft Hortense Desktop|fr-FR|Female\r\n",
        );
        assert_eq!(voices.len(), 4);
        assert_eq!(
            pick(&voices, None).as_deref(),
            Some("Microsoft Zira Desktop")
        );
        assert_eq!(
            pick(&voices, Some("male")).as_deref(),
            Some("Microsoft David Desktop")
        );
        assert_eq!(
            pick(&voices, Some("hazel")).as_deref(),
            Some("Microsoft Hazel Desktop")
        );
        assert_eq!(
            pick(&voices, Some("Microsoft Hortense Desktop")).as_deref(),
            Some("Microsoft Hortense Desktop")
        );
        assert_eq!(pick(&[], None), None);
    }
}
