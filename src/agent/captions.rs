//! Captions for speech the agent made: the script is known, so there is
//! nothing to recognise, only to time. The words are cut into short
//! on-screen chunks (a few words, never across a sentence), and each
//! chunk's time is its share of the spoken span, snapped to the pauses the
//! recording really has, so a caption changes when the voice breathes.

use super::voice::Measured;
use crate::model::Micros;

/// A chunk of words and when it's on screen, in the speech file's time.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub text: String,
    pub start: Micros,
    pub end: Micros,
}

/// Words per on-screen caption (short-form video reads in small bites).
pub const MAX_WORDS: usize = 5;
const MAX_CHARS: usize = 28;

fn ends_sentence(word: &str) -> bool {
    word.trim_end_matches(['"', '\'', ')', '”', '’'])
        .ends_with(['.', '!', '?', '…'])
}

fn ends_phrase(word: &str) -> bool {
    word.ends_with([',', ';', ':', '—', '–'])
}

/// The script cut into captions: at most `max_words` words / ~28 letters,
/// a sentence's end always a break, a comma one once there are three words.
/// A lone last word joins the chunk before it.
pub fn chunks(text: &str, max_words: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<Vec<&str>> = vec![];
    let mut cur: Vec<&str> = vec![];
    let len = |c: &[&str]| c.iter().map(|w| w.chars().count() + 1).sum::<usize>();
    for (i, w) in words.iter().enumerate() {
        if !cur.is_empty()
            && (cur.len() >= max_words.max(1) || len(&cur) + w.chars().count() > MAX_CHARS)
        {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(w);
        let last = i + 1 == words.len();
        if !last && (ends_sentence(w) || (ends_phrase(w) && cur.len() >= 3)) {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    // "…and save. / Money." reads badly: a lone trailing word joins in,
    // unless a sentence ended before it.
    if out.len() >= 2 && out[out.len() - 1].len() == 1 {
        let prev = &out[out.len() - 2];
        if !prev.last().is_some_and(|w| ends_sentence(w)) && prev.len() <= max_words {
            let last = out.pop().unwrap_or_default();
            if let Some(p) = out.last_mut() {
                p.extend(last);
            }
        }
    }
    out.into_iter().map(|c| c.join(" ")).collect()
}

/// How long a chunk takes to say, relatively: its letters, plus a beat
/// for the pause after a phrase or sentence.
fn weight(chunk: &str) -> f64 {
    let letters = chunk.chars().filter(|c| c.is_alphanumeric()).count() as f64;
    let words = chunk.split_whitespace().count() as f64;
    let pause = if ends_sentence(chunk) {
        6.
    } else if ends_phrase(chunk) {
        3.
    } else {
        0.
    };
    letters + words * 1.5 + pause
}

/// Times `chunks` over the speech `m`: proportional over its audible span,
/// each break snapped into the nearest real pause when one is close.
pub fn time_cues(chunks: &[String], m: &Measured) -> Vec<Cue> {
    if chunks.is_empty() {
        return vec![];
    }
    let span = (m.tail - m.onset).max(1) as f64;
    let total: f64 = chunks.iter().map(|c| weight(c)).sum::<f64>().max(1.);
    let mut used = vec![false; m.pauses.len()];
    let mut starts = vec![m.onset];
    let mut ends = vec![];
    let mut acc = 0.;
    for (i, c) in chunks.iter().enumerate().take(chunks.len() - 1) {
        acc += weight(c);
        let guess = m.onset + (acc / total * span) as Micros;
        let chunk_len = (weight(c) / total * span) as Micros;
        let reach = (chunk_len / 2).max(350_000);
        let prev = starts[i];
        let best = m
            .pauses
            .iter()
            .enumerate()
            .filter(|(j, (a, _))| !used[*j] && *a > prev)
            .map(|(j, (a, b))| (j, (a + b) / 2, *a, *b))
            .filter(|(_, mid, _, _)| (mid - guess).abs() <= reach)
            .min_by_key(|(_, mid, _, _)| (mid - guess).abs());
        match best {
            Some((j, _, a, b)) => {
                used[j] = true;
                ends.push(a);
                starts.push(b);
            }
            None => {
                let at = guess.max(prev + 1);
                ends.push(at);
                starts.push(at);
            }
        }
    }
    ends.push(m.tail);
    // On screen until the next one starts (no blink between), the last a
    // little past the voice.
    chunks
        .iter()
        .enumerate()
        .map(|(i, text)| Cue {
            text: text.clone(),
            start: starts[i],
            end: if i + 1 < chunks.len() {
                starts[i + 1]
            } else {
                (ends[i] + 250_000).min(m.length.max(ends[i]))
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_short_chunks_at_sentences_and_phrases() {
        let c = chunks(
            "Do you know where your money goes every day? Coffee, snacks, rides. It adds up.",
            5,
        );
        assert_eq!(
            c,
            vec![
                "Do you know where your",
                "money goes every day?",
                "Coffee, snacks, rides.",
                "It adds up."
            ]
        );
    }

    #[test]
    fn a_lone_last_word_joins_the_chunk_before() {
        let c = chunks("Small choices today build a better tomorrow", 5);
        assert_eq!(c, vec!["Small choices today build a", "better tomorrow"]);
        let c = chunks("one two three four five six", 5);
        assert_eq!(c, vec!["one two three four five six"]);
    }

    #[test]
    fn breaks_snap_to_real_pauses() {
        let m = Measured {
            length: 3_000_000,
            onset: 100_000,
            tail: 2_900_000,
            pauses: vec![(1_300_000, 1_600_000)],
        };
        let cues = time_cues(&["Save a little.".into(), "Every day.".into()], &m);
        assert_eq!(cues[0].start, 100_000);
        assert_eq!(cues[1].start, 1_600_000);
        assert_eq!(cues[0].end, 1_600_000, "held until the next one");
        assert_eq!(cues[1].end, 3_000_000);
    }

    #[test]
    fn without_pauses_time_is_shared_by_length() {
        let m = Measured {
            length: 2_000_000,
            onset: 0,
            tail: 2_000_000,
            pauses: vec![],
        };
        let cues = time_cues(&["aaaa".into(), "aaaa".into()], &m);
        assert_eq!(cues[1].start, 1_000_000);
        assert!(cues.windows(2).all(|w| w[0].end <= w[1].start + 1));
    }
}
