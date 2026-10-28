//! Subtitles: SRT ↔ VTT with cue timings preserved, and a timing-free TXT
//! extraction. Pure Rust, no external tool, so subtitle conversion works even
//! when every bundled binary is missing.

use crate::error::{ErrorCode, JobError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    /// Original cue number, or the ordinal when the source had none.
    pub index: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Text with internal newlines preserved.
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subtitle {
    pub cues: Vec<Cue>,
}

impl Subtitle {
    pub fn parse(input: &str) -> Result<Self, JobError> {
        let body = input.trim_start_matches('\u{feff}');
        let mut cues = Vec::new();
        let mut lines = body.lines().peekable();

        // Skip an optional WEBVTT header and its metadata block.
        if lines
            .peek()
            .map(|l| l.trim_start().starts_with("WEBVTT"))
            .unwrap_or(false)
        {
            lines.next();
            while let Some(l) = lines.peek() {
                if l.trim().is_empty() {
                    lines.next();
                    break;
                }
                lines.next();
            }
        }

        let mut ordinal = 0u32;
        while let Some(line) = lines.next() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // A cue may or may not carry a numeric index line.
            let (index, timing_line) = if trimmed.contains("-->") {
                ordinal += 1;
                (ordinal, trimmed.to_string())
            } else {
                ordinal += 1;
                let idx = trimmed.parse::<u32>().unwrap_or(ordinal);
                match lines.next() {
                    Some(t) if t.contains("-->") => (idx, t.trim().to_string()),
                    _ => continue,
                }
            };
            let Some((start_ms, end_ms)) = parse_timing(&timing_line) else {
                continue;
            };
            let mut text_lines: Vec<String> = Vec::new();
            while let Some(next) = lines.peek() {
                if next.trim().is_empty() {
                    break;
                }
                text_lines.push(lines.next().unwrap().to_string());
            }
            // A cue number directly followed by another cue number (no blank
            // separator) is malformed; the text buffer is simply empty then.
            cues.push(Cue {
                index,
                start_ms,
                end_ms,
                text: text_lines.join("\n"),
            });
        }

        if cues.is_empty() {
            return Err(JobError::new(
                ErrorCode::EngineFailed,
                "no subtitle cues found",
            ));
        }
        Ok(Subtitle { cues })
    }

    pub fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, cue) in self.cues.iter().enumerate() {
            out.push_str(&format!("{}\n", i + 1));
            out.push_str(&format!(
                "{} --> {}\n{}\n\n",
                fmt_time(cue.start_ms, ',', true),
                fmt_time(cue.end_ms, ',', true),
                cue.text
            ));
        }
        out
    }

    pub fn to_vtt(&self) -> String {
        let mut out = String::from("WEBVTT\n\n");
        for cue in &self.cues {
            out.push_str(&format!(
                "{} --> {}\n{}\n\n",
                fmt_time(cue.start_ms, '.', true),
                fmt_time(cue.end_ms, '.', true),
                cue.text
            ));
        }
        out
    }

    /// Plain text: timings and cue numbers dropped, one cue per paragraph.
    pub fn to_txt(&self) -> String {
        let mut out = String::new();
        for cue in &self.cues {
            out.push_str(&cue.text);
            out.push('\n');
        }
        out
    }

    pub fn to_document(&self, ext: &str) -> Result<String, JobError> {
        match ext {
            "srt" => Ok(self.to_srt()),
            "vtt" => Ok(self.to_vtt()),
            "txt" => Ok(self.to_txt()),
            other => Err(JobError::unsupported("subtitles", other)),
        }
    }
}

fn parse_timing(line: &str) -> Option<(u64, u64)> {
    let (start, rest) = line.split_once("-->")?;
    // Strip any cue settings after the end timestamp.
    let end = rest.split_whitespace().next()?;
    Some((parse_time(start.trim())?, parse_time(end.trim())?))
}

fn parse_time(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, rest) = match parts.as_slice() {
        [h, m, r] => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?, *r),
        [m, r] => (0, m.parse::<u64>().ok()?, *r),
        _ => return None,
    };
    let normalized = rest.replace(',', ".");
    let (sec, frac) = normalized
        .split_once('.')
        .unwrap_or((normalized.as_str(), "0"));
    let sec: u64 = sec.parse().ok()?;
    let mut frac_digits = frac.to_string();
    while frac_digits.len() < 3 {
        frac_digits.push('0');
    }
    let ms: u64 = frac_digits[..3].parse().ok()?;
    Some(h * 3_600_000 + m * 60_000 + sec * 1_000 + ms)
}

fn fmt_time(ms: u64, sep: char, with_hours: bool) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let millis = ms % 1000;
    if with_hours || h > 0 {
        format!("{h:02}:{m:02}:{s:02}{sep}{millis:03}")
    } else {
        format!("{m:02}:{s:02}{sep}{millis:03}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:04,000\nHello there\n\n2\n00:00:05,500 --> 00:00:07,250\nSecond line\nkeeps its break\n\n";

    #[test]
    fn srt_vtt_roundtrip_preserves_cues() {
        let srt = Subtitle::parse(SRT).unwrap();
        assert_eq!(srt.cues.len(), 2);
        assert_eq!(srt.cues[0].start_ms, 1000);
        assert_eq!(srt.cues[1].end_ms, 7250);
        assert_eq!(srt.cues[1].text, "Second line\nkeeps its break");

        let vtt_text = srt.to_vtt();
        assert!(vtt_text.starts_with("WEBVTT\n\n"));
        let vtt = Subtitle::parse(&vtt_text).unwrap();
        assert_eq!(vtt.cues, srt.cues);

        let back = Subtitle::parse(&vtt.to_srt()).unwrap();
        assert_eq!(back.cues, srt.cues);
        assert_eq!(back.to_srt(), srt.to_srt());
    }

    #[test]
    fn txt_strips_timing() {
        let srt = Subtitle::parse(SRT).unwrap();
        let txt = srt.to_txt();
        assert!(!txt.contains("-->"));
        assert!(txt.contains("Hello there"));
        assert!(txt.contains("keeps its break"));
    }

    #[test]
    fn empty_input_is_an_engine_failure_not_a_panic() {
        let err = Subtitle::parse("").unwrap_err();
        assert_eq!(err.code, ErrorCode::EngineFailed);
        assert!(!err.message.is_empty());
    }

    #[test]
    fn vtt_with_cue_settings_and_short_timestamps_parses() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000 align:start position:10%\nHi\n";
        let s = Subtitle::parse(vtt).unwrap();
        assert_eq!(s.cues[0].start_ms, 1000);
        assert_eq!(s.cues[0].end_ms, 2000);
        assert_eq!(s.cues[0].text, "Hi");
    }
}
