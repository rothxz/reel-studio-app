//! Line icons for diagram nodes and comparison columns, drawn as paths in
//! a unit square (0–1) and stroked in the node's colour. A node names one,
//! or one is guessed from its words ("vector store" → database, "emotions"
//! → heart).
//!
//! The guess reads the whole scene first: in a technical one (code, APIs,
//! models…) "memory" is a database; anywhere else the tech-only icons are
//! never guessed, so a video about the brain doesn't get circuit boards.

use tiny_skia::{FillRule, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use super::{LineCap, Rgb, paint, rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Chat,
    User,
    Search,
    Database,
    Doc,
    Chip,
    Check,
    Code,
    Cloud,
    Lock,
    Coin,
    Chart,
    Gear,
    Bolt,
    Globe,
    Bulb,
    Mail,
    Clock,
    Phone,
    Star,
    Sparkle,
    Play,
    Brain,
    Heart,
    Smile,
    Eye,
    Ear,
    Hand,
    Target,
    Network,
    Book,
    Cap,
    Leaf,
    Sun,
    Moon,
    Home,
    Music,
    Puzzle,
    Rocket,
    Trophy,
    Apple,
    Drop,
    Pin,
    Shield,
}

/// Each icon: the name the model may use, whether it only suits technical
/// scenes, and words that suggest it (a word in two lists: the scene's
/// kind breaks the tie, else the first one).
const TABLE: &[(Icon, &str, bool, &[&str])] = &[
    (
        Icon::Chat,
        "chat",
        false,
        &[
            "query",
            "question",
            "ask",
            "prompt",
            "chat",
            "message",
            "comment",
            "talk",
            "conversation",
            "speak",
            "say",
        ],
    ),
    (
        Icon::User,
        "user",
        false,
        &[
            "user", "customer", "person", "people", "team", "client", "you", "audience", "body",
            "self", "human",
        ],
    ),
    (
        Icon::Brain,
        "brain",
        false,
        &[
            "brain",
            "mind",
            "neuro",
            "cognit",
            "mental",
            "intellig",
            "memor",
            "remember",
            "recall",
            "psycholog",
            "conscious",
            "smart",
        ],
    ),
    (
        Icon::Heart,
        "heart",
        false,
        &[
            "heart",
            "love",
            "emotion",
            "feel",
            "care",
            "health",
            "relationship",
            "passion",
            "cardio",
            "blood",
            "empathy",
        ],
    ),
    (
        Icon::Smile,
        "smile",
        false,
        &[
            "happy",
            "happi",
            "joy",
            "smile",
            "mood",
            "fun",
            "laugh",
            "friend",
            "wellbeing",
            "well-being",
            "emotion",
            "confiden",
            "positive",
        ],
    ),
    (
        Icon::Eye,
        "eye",
        false,
        &[
            "eye",
            "see",
            "sight",
            "vision",
            "look",
            "observe",
            "notice",
            "sens",
            "perceiv",
            "receiv",
            "attention",
            "aware",
            "visual",
        ],
    ),
    (
        Icon::Ear,
        "ear",
        false,
        &["ear", "hear", "listen", "sound", "audio", "noise", "voice"],
    ),
    (
        Icon::Hand,
        "hand",
        false,
        &[
            "hand",
            "touch",
            "action",
            "act",
            "do",
            "help",
            "give",
            "hold",
            "respon",
            "move",
            "practice",
            "practise",
            "volunteer",
            "support",
            "body",
        ],
    ),
    (
        Icon::Target,
        "target",
        false,
        &[
            "goal", "target", "aim", "focus", "decid", "decision", "choose", "choice", "strategy",
            "mission", "purpose", "priorit", "result",
        ],
    ),
    (
        Icon::Network,
        "network",
        false,
        &[
            "connect",
            "network",
            "link",
            "signal",
            "neuron",
            "nerve",
            "synap",
            "relation",
            "social",
            "community",
            "share",
            "communicat",
        ],
    ),
    (
        Icon::Book,
        "book",
        false,
        &[
            "book",
            "read",
            "learn",
            "study",
            "lesson",
            "knowledge",
            "educat",
            "teach",
            "story",
            "fact",
            "memor",
            "store",
            "experience",
            "skill",
        ],
    ),
    (
        Icon::Cap,
        "graduation",
        false,
        &[
            "graduat",
            "student",
            "degree",
            "universit",
            "college",
            "school",
            "course",
            "exam",
            "class",
        ],
    ),
    (
        Icon::Leaf,
        "leaf",
        false,
        &[
            "plant",
            "nature",
            "leaf",
            "tree",
            "green",
            "eco",
            "environment",
            "forest",
            "garden",
            "organic",
            "climate",
            "natural",
            "vegan",
            "grow",
            "bee",
            "flower",
            "pollinat",
            "crop",
            "farm",
            "insect",
            "animal",
        ],
    ),
    (
        Icon::Sun,
        "sun",
        false,
        &[
            "sun", "day", "morning", "light", "summer", "solar", "warm", "bright", "weather",
            "wake",
        ],
    ),
    (
        Icon::Moon,
        "moon",
        false,
        &[
            "sleep", "night", "rest", "dream", "moon", "relax", "bed", "evening", "calm", "recover",
        ],
    ),
    (
        Icon::Home,
        "home",
        false,
        &["home", "house", "family", "room", "stay", "rent", "kitchen"],
    ),
    (
        Icon::Music,
        "music",
        false,
        &[
            "music", "song", "sing", "melody", "rhythm", "beat", "playlist", "dance", "band",
        ],
    ),
    (
        Icon::Puzzle,
        "puzzle",
        false,
        &[
            "problem",
            "puzzle",
            "solve",
            "piece",
            "part",
            "fit",
            "combine",
            "challenge",
            "together",
            "game",
        ],
    ),
    (
        Icon::Rocket,
        "rocket",
        false,
        &[
            "launch",
            "start",
            "begin",
            "rocket",
            "boost",
            "scale",
            "space",
            "accelerat",
            "ship",
            "takeoff",
            "first step",
        ],
    ),
    (
        Icon::Trophy,
        "trophy",
        false,
        &[
            "win", "success", "achiev", "award", "champion", "reward", "prize", "compet",
            "victory", "master",
        ],
    ),
    (
        Icon::Apple,
        "apple",
        false,
        &[
            "food",
            "eat",
            "diet",
            "nutrition",
            "fruit",
            "meal",
            "apple",
            "snack",
            "cook",
            "vitamin",
            "healthy",
            "breakfast",
            "lunch",
            "dinner",
        ],
    ),
    (
        Icon::Drop,
        "drop",
        false,
        &[
            "water", "drink", "hydrat", "rain", "liquid", "ocean", "sea", "river", "wash", "clean",
        ],
    ),
    (
        Icon::Pin,
        "pin",
        false,
        &[
            "place",
            "location",
            "map",
            "travel",
            "visit",
            "city",
            "where",
            "destination",
            "trip",
            "local",
        ],
    ),
    (
        Icon::Shield,
        "shield",
        false,
        &[
            "protect", "defen", "immune", "guard", "safe", "insur", "risk", "prevent", "shield",
            "resist",
        ],
    ),
    (
        Icon::Search,
        "search",
        false,
        &[
            "search", "retriev", "find", "lookup", "look up", "scan", "explore", "discover",
            "research", "curious",
        ],
    ),
    (
        Icon::Database,
        "database",
        true,
        &[
            "database",
            "data",
            "store",
            "storage",
            "vector",
            "knowledge",
            "memory",
            "cache",
            "db",
            "index",
        ],
    ),
    (
        Icon::Doc,
        "doc",
        false,
        &[
            "doc", "file", "context", "page", "pdf", "report", "augment", "note", "article",
            "contract", "text", "write", "list",
        ],
    ),
    (
        Icon::Chip,
        "chip",
        true,
        &[
            "llm", "model", "ai", "gpt", "neural", "cpu", "gpu", "compute", "generat", "reason",
        ],
    ),
    (
        Icon::Check,
        "check",
        false,
        &[
            "answer", "result", "output", "done", "verify", "approve", "finish", "grounded",
            "complete", "correct", "true",
        ],
    ),
    (
        Icon::Code,
        "code",
        true,
        &[
            "code",
            "api",
            "function",
            "script",
            "dev",
            "program",
            "sdk",
            "python",
            "rust",
            "javascript",
        ],
    ),
    (
        Icon::Cloud,
        "cloud",
        true,
        &["cloud", "server", "deploy", "host", "upload", "sync"],
    ),
    (
        Icon::Lock,
        "lock",
        false,
        &[
            "secur",
            "auth",
            "lock",
            "privacy",
            "password",
            "encrypt",
            "permission",
            "secret",
        ],
    ),
    (
        Icon::Coin,
        "coin",
        false,
        &[
            "money", "pay", "price", "cost", "budget", "save", "$", "invest", "coin", "salary",
            "income", "spend", "profit", "revenue", "cash", "debt",
        ],
    ),
    (
        Icon::Chart,
        "chart",
        false,
        &[
            "grow", "chart", "metric", "analytic", "stat", "sales", "trend", "measure", "kpi",
            "rank", "progress", "improve",
        ],
    ),
    (
        Icon::Gear,
        "gear",
        false,
        &[
            "setting", "config", "process", "pipeline", "engine", "tool", "system", "build",
            "automat", "workflow", "work", "machine", "method",
        ],
    ),
    (
        Icon::Bolt,
        "bolt",
        false,
        &[
            "fast", "speed", "power", "energy", "instant", "quick", "trigger", "event", "electric",
            "react", "reflex",
        ],
    ),
    (
        Icon::Globe,
        "globe",
        false,
        &[
            "web", "internet", "world", "global", "online", "browser", "site", "earth", "country",
            "planet", "culture", "language",
        ],
    ),
    (
        Icon::Bulb,
        "bulb",
        false,
        &[
            "idea",
            "insight",
            "tip",
            "think",
            "thought",
            "plan",
            "understand",
            "creativ",
            "imagin",
            "solution",
            "invent",
            "clever",
        ],
    ),
    (
        Icon::Mail,
        "mail",
        false,
        &[
            "mail",
            "email",
            "send",
            "inbox",
            "newsletter",
            "notify",
            "letter",
        ],
    ),
    (
        Icon::Clock,
        "clock",
        false,
        &[
            "time", "wait", "schedul", "deadline", "minute", "hour", "daily", "later", "history",
            "habit", "routine", "past", "future",
        ],
    ),
    (
        Icon::Phone,
        "phone",
        false,
        &["phone", "mobile", "app", "device", "screen", "call"],
    ),
    (
        Icon::Star,
        "star",
        false,
        &[
            "star",
            "best",
            "favorite",
            "favourite",
            "rating",
            "review",
            "quality",
            "special",
            "famous",
        ],
    ),
    (
        Icon::Play,
        "play",
        false,
        &[
            "video", "play", "watch", "stream", "media", "reel", "movie", "film",
        ],
    ),
    (
        Icon::Sparkle,
        "sparkle",
        false,
        &[
            "magic", "sparkle", "new", "create", "polish", "wonder", "surprise",
        ],
    ),
];

/// Words that only come up when a topic is technical (software, AI,
/// infrastructure): one makes the scene technical.
const TECH_WORDS: &[&str] = &[
    "api",
    "apis",
    "code",
    "coding",
    "server",
    "servers",
    "database",
    "databases",
    "db",
    "llm",
    "llms",
    "gpt",
    "ai",
    "ml",
    "gpu",
    "cpu",
    "sdk",
    "cloud",
    "deploy",
    "deployment",
    "vector",
    "embedding",
    "embeddings",
    "python",
    "rust",
    "javascript",
    "typescript",
    "algorithm",
    "algorithms",
    "software",
    "backend",
    "frontend",
    "http",
    "json",
    "sql",
    "docker",
    "kubernetes",
    "git",
    "repo",
    "compiler",
    "cache",
    "rag",
    "token",
    "tokens",
    "endpoint",
    "devops",
    "microservice",
    "microservices",
    "query",
    "prompt",
    "retriever",
    "inference",
    "binary",
    "html",
    "css",
    "chatbot",
    "agent",
    "transformer",
    "neural",
];

/// Whether words read as a technical topic (software, AI, infrastructure).
pub fn technical(text: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| TECH_WORDS.contains(&w))
}

impl Icon {
    pub fn names() -> Vec<&'static str> {
        TABLE.iter().map(|(_, n, _, _)| *n).collect()
    }

    pub fn parse(name: &str) -> Option<Icon> {
        let n = name.trim().to_lowercase();
        TABLE
            .iter()
            .find(|(_, k, _, _)| *k == n)
            .map(|(i, _, _, _)| *i)
    }

    /// The icon a node's words suggest (the label counts more than the
    /// detail), in a technical scene or not.
    #[cfg(test)]
    pub fn guess(label: &str, detail: &str, tech: bool) -> Icon {
        Self::ranked(label, detail, tech)
            .first()
            .copied()
            .unwrap_or(Icon::Sparkle)
    }

    /// The icon `guess` would pick, or None when no word suggests one.
    pub fn suggest(label: &str, detail: &str, tech: bool) -> Option<Icon> {
        Self::ranked(label, detail, tech).first().copied()
    }

    /// Icons for a row of labels (a diagram's nodes): each its best guess,
    /// but the same picture twice only when nothing else fits ("Brain",
    /// "Memory" → brain, book). `given`: icons the model named.
    pub fn guess_all(items: &[(&str, &str, Option<Icon>)], tech: bool) -> Vec<Icon> {
        let mut used: Vec<Icon> = items.iter().filter_map(|(_, _, i)| *i).collect();
        items
            .iter()
            .map(|(label, detail, given)| {
                if let Some(i) = given {
                    return *i;
                }
                let ranked = Self::ranked(label, detail, tech);
                let pick = ranked
                    .iter()
                    .find(|i| !used.contains(i))
                    .or(ranked.first())
                    .copied()
                    .unwrap_or(Icon::Sparkle);
                used.push(pick);
                pick
            })
            .collect()
    }

    /// Every icon some word suggests, best first.
    fn ranked(label: &str, detail: &str, tech: bool) -> Vec<Icon> {
        let score = |text: &str, words: &[&str]| {
            let t = format!(" {} ", text.to_lowercase());
            words
                .iter()
                .filter(|w| {
                    // Short words must stand alone ("ai" isn't "maintain").
                    if w.len() <= 3 {
                        t.split(|c: char| !c.is_alphanumeric() && c != '$')
                            .any(|x| x == **w)
                    } else {
                        t.contains(*w)
                    }
                })
                .count()
        };
        let mut found: Vec<(usize, usize, Icon)> = TABLE
            .iter()
            .enumerate()
            // Circuit boards and databases only where the topic is technical.
            .filter(|(_, (_, _, tech_only, _))| tech || !tech_only)
            .map(|(k, (i, _, tech_only, words))| {
                let s = score(label, words) * 3 + score(detail, words);
                // A tie: a technical scene's own icon first.
                (s * 2 + usize::from(tech && *tech_only), k, *i)
            })
            .filter(|(s, _, _)| *s > 1)
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        found.into_iter().map(|(_, _, i)| i).collect()
    }

    /// The outlines, in a unit square; filled ones are dots.
    fn paths(self) -> (Vec<Path>, Vec<Path>) {
        let mut strokes = vec![];
        let mut fills = vec![];
        let poly = |pts: &[(f32, f32)], close: bool| {
            let mut pb = PathBuilder::new();
            pb.move_to(pts[0].0, pts[0].1);
            for p in &pts[1..] {
                pb.line_to(p.0, p.1);
            }
            if close {
                pb.close();
            }
            pb.finish()
        };
        let circle = |x: f32, y: f32, r: f32| PathBuilder::from_circle(x, y, r);
        let quad = |a: (f32, f32), c: (f32, f32), b: (f32, f32)| {
            let mut pb = PathBuilder::new();
            pb.move_to(a.0, a.1);
            pb.quad_to(c.0, c.1, b.0, b.1);
            pb.finish()
        };
        let rect = |l: f32, t: f32, r: f32, b: f32, round: f32| {
            Rect::from_ltrb(l, t, r, b).and_then(|r| crate::text_render::rounded(r, round))
        };
        let mut s = |p: Option<Path>| strokes.extend(p);
        match self {
            Icon::Chat => {
                s(rect(0.1, 0.14, 0.9, 0.7, 0.35));
                s(poly(&[(0.3, 0.7), (0.24, 0.88), (0.48, 0.7)], false));
                for x in [0.33, 0.5, 0.67] {
                    fills.extend(circle(x, 0.42, 0.05));
                }
            }
            Icon::User => {
                s(circle(0.5, 0.33, 0.17));
                let mut pb = PathBuilder::new();
                pb.move_to(0.16, 0.88);
                pb.cubic_to(0.16, 0.58, 0.84, 0.58, 0.84, 0.88);
                s(pb.finish());
            }
            Icon::Search => {
                s(circle(0.42, 0.42, 0.26));
                s(poly(&[(0.61, 0.61), (0.86, 0.86)], false));
            }
            Icon::Database => {
                s(Rect::from_ltrb(0.18, 0.12, 0.82, 0.32)
                    .map(PathBuilder::from_oval)
                    .flatten());
                for y in [0.5, 0.78] {
                    let mut pb = PathBuilder::new();
                    pb.move_to(0.18, y);
                    pb.cubic_to(0.18, y + 0.13, 0.82, y + 0.13, 0.82, y);
                    s(pb.finish());
                }
                s(poly(&[(0.18, 0.22), (0.18, 0.78)], false));
                s(poly(&[(0.82, 0.22), (0.82, 0.78)], false));
            }
            Icon::Doc => {
                s(poly(
                    &[(0.22, 0.1), (0.6, 0.1), (0.8, 0.3), (0.8, 0.9), (0.22, 0.9)],
                    true,
                ));
                s(poly(&[(0.6, 0.1), (0.6, 0.3), (0.8, 0.3)], false));
                for y in [0.5, 0.62, 0.74] {
                    s(poly(&[(0.34, y), (0.68, y)], false));
                }
            }
            Icon::Chip => {
                s(rect(0.25, 0.25, 0.75, 0.75, 0.25));
                s(rect(0.39, 0.39, 0.61, 0.61, 0.2));
                for k in [0.36, 0.5, 0.64] {
                    s(poly(&[(k, 0.1), (k, 0.25)], false));
                    s(poly(&[(k, 0.75), (k, 0.9)], false));
                    s(poly(&[(0.1, k), (0.25, k)], false));
                    s(poly(&[(0.75, k), (0.9, k)], false));
                }
            }
            Icon::Check => {
                s(circle(0.5, 0.5, 0.38));
                s(poly(&[(0.32, 0.52), (0.45, 0.65), (0.7, 0.38)], false));
            }
            Icon::Code => {
                s(poly(&[(0.34, 0.28), (0.12, 0.5), (0.34, 0.72)], false));
                s(poly(&[(0.66, 0.28), (0.88, 0.5), (0.66, 0.72)], false));
                s(poly(&[(0.57, 0.2), (0.43, 0.8)], false));
            }
            Icon::Cloud => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.28, 0.74);
                pb.line_to(0.74, 0.74);
                pb.cubic_to(0.9, 0.74, 0.94, 0.52, 0.8, 0.46);
                pb.cubic_to(0.8, 0.28, 0.58, 0.22, 0.5, 0.36);
                pb.cubic_to(0.42, 0.26, 0.24, 0.3, 0.27, 0.47);
                pb.cubic_to(0.12, 0.5, 0.12, 0.74, 0.28, 0.74);
                pb.close();
                s(pb.finish());
            }
            Icon::Lock => {
                s(rect(0.22, 0.45, 0.78, 0.88, 0.25));
                let mut pb = PathBuilder::new();
                pb.move_to(0.34, 0.45);
                pb.line_to(0.34, 0.32);
                pb.cubic_to(0.34, 0.1, 0.66, 0.1, 0.66, 0.32);
                pb.line_to(0.66, 0.45);
                s(pb.finish());
                fills.extend(circle(0.5, 0.64, 0.055));
            }
            Icon::Coin => {
                s(circle(0.5, 0.5, 0.38));
                let mut pb = PathBuilder::new();
                pb.move_to(0.61, 0.37);
                pb.cubic_to(0.56, 0.3, 0.39, 0.3, 0.39, 0.41);
                pb.cubic_to(0.39, 0.51, 0.61, 0.48, 0.61, 0.6);
                pb.cubic_to(0.61, 0.71, 0.44, 0.71, 0.38, 0.63);
                s(pb.finish());
                s(poly(&[(0.5, 0.23), (0.5, 0.77)], false));
            }
            Icon::Chart => {
                s(poly(&[(0.14, 0.12), (0.14, 0.86), (0.88, 0.86)], false));
                s(poly(
                    &[(0.24, 0.68), (0.42, 0.5), (0.56, 0.6), (0.82, 0.3)],
                    false,
                ));
                s(poly(&[(0.66, 0.3), (0.82, 0.3), (0.82, 0.46)], false));
            }
            Icon::Gear => {
                s(circle(0.5, 0.5, 0.15));
                s(circle(0.5, 0.5, 0.3));
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    let (c, si) = (a.cos(), a.sin());
                    s(poly(
                        &[
                            (0.5 + c * 0.3, 0.5 + si * 0.3),
                            (0.5 + c * 0.42, 0.5 + si * 0.42),
                        ],
                        false,
                    ));
                }
            }
            Icon::Bolt => {
                s(poly(
                    &[
                        (0.56, 0.08),
                        (0.24, 0.54),
                        (0.48, 0.54),
                        (0.42, 0.92),
                        (0.76, 0.42),
                        (0.52, 0.42),
                    ],
                    true,
                ));
            }
            Icon::Globe => {
                s(circle(0.5, 0.5, 0.38));
                s(Rect::from_ltrb(0.33, 0.12, 0.67, 0.88)
                    .map(PathBuilder::from_oval)
                    .flatten());
                s(poly(&[(0.12, 0.5), (0.88, 0.5)], false));
                s(poly(&[(0.2, 0.31), (0.8, 0.31)], false));
                s(poly(&[(0.2, 0.69), (0.8, 0.69)], false));
            }
            Icon::Bulb => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.37, 0.66);
                pb.cubic_to(0.2, 0.54, 0.22, 0.12, 0.5, 0.12);
                pb.cubic_to(0.78, 0.12, 0.8, 0.54, 0.63, 0.66);
                pb.line_to(0.63, 0.76);
                pb.line_to(0.37, 0.76);
                pb.close();
                s(pb.finish());
                s(poly(&[(0.41, 0.87), (0.59, 0.87)], false));
            }
            Icon::Mail => {
                s(rect(0.12, 0.24, 0.88, 0.76, 0.12));
                s(poly(&[(0.14, 0.28), (0.5, 0.54), (0.86, 0.28)], false));
            }
            Icon::Clock => {
                s(circle(0.5, 0.5, 0.38));
                s(poly(&[(0.5, 0.27), (0.5, 0.5), (0.67, 0.6)], false));
            }
            Icon::Phone => {
                s(rect(0.3, 0.1, 0.7, 0.9, 0.25));
                s(poly(&[(0.45, 0.79), (0.55, 0.79)], false));
            }
            Icon::Star => {
                let pts: Vec<(f32, f32)> = (0..10)
                    .map(|k| {
                        let r = if k % 2 == 0 { 0.4 } else { 0.17 };
                        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.;
                        (0.5 + r * a.cos(), 0.53 + r * a.sin())
                    })
                    .collect();
                s(poly(&pts, true));
            }
            Icon::Sparkle => {
                let star = |cx: f32, cy: f32, r: f32| {
                    let k = r * 0.22;
                    poly(
                        &[
                            (cx, cy - r),
                            (cx + k, cy - k),
                            (cx + r, cy),
                            (cx + k, cy + k),
                            (cx, cy + r),
                            (cx - k, cy + k),
                            (cx - r, cy),
                            (cx - k, cy - k),
                        ],
                        true,
                    )
                };
                s(star(0.42, 0.56, 0.32));
                s(star(0.76, 0.24, 0.14));
            }
            Icon::Play => {
                s(circle(0.5, 0.5, 0.38));
                s(poly(&[(0.42, 0.33), (0.68, 0.5), (0.42, 0.67)], true));
            }
            Icon::Brain => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.2);
                pb.cubic_to(0.42, 0.1, 0.24, 0.12, 0.24, 0.27);
                pb.cubic_to(0.1, 0.3, 0.08, 0.48, 0.17, 0.55);
                pb.cubic_to(0.1, 0.66, 0.18, 0.82, 0.32, 0.8);
                pb.cubic_to(0.36, 0.9, 0.48, 0.9, 0.5, 0.82);
                pb.cubic_to(0.52, 0.9, 0.64, 0.9, 0.68, 0.8);
                pb.cubic_to(0.82, 0.82, 0.9, 0.66, 0.83, 0.55);
                pb.cubic_to(0.92, 0.48, 0.9, 0.3, 0.76, 0.27);
                pb.cubic_to(0.76, 0.12, 0.58, 0.1, 0.5, 0.2);
                pb.close();
                s(pb.finish());
                s(poly(&[(0.5, 0.2), (0.5, 0.82)], false));
                // The folds, mirrored.
                for m in [1., -1.] {
                    let x = |v: f32| 0.5 + m * (v - 0.5);
                    s(quad((x(0.24), 0.27), (x(0.26), 0.38), (x(0.38), 0.4)));
                    s(quad((x(0.17), 0.55), (x(0.28), 0.5), (x(0.36), 0.56)));
                    s(quad((x(0.32), 0.8), (x(0.3), 0.7), (x(0.4), 0.66)));
                }
            }
            Icon::Heart => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.84);
                pb.cubic_to(0.2, 0.62, 0.06, 0.42, 0.18, 0.26);
                pb.cubic_to(0.29, 0.12, 0.46, 0.16, 0.5, 0.3);
                pb.cubic_to(0.54, 0.16, 0.71, 0.12, 0.82, 0.26);
                pb.cubic_to(0.94, 0.42, 0.8, 0.62, 0.5, 0.84);
                pb.close();
                s(pb.finish());
            }
            Icon::Smile => {
                s(circle(0.5, 0.5, 0.38));
                fills.extend(circle(0.38, 0.41, 0.045));
                fills.extend(circle(0.62, 0.41, 0.045));
                s(quad((0.33, 0.58), (0.5, 0.76), (0.67, 0.58)));
            }
            Icon::Eye => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.08, 0.5);
                pb.quad_to(0.5, 0.1, 0.92, 0.5);
                pb.quad_to(0.5, 0.9, 0.08, 0.5);
                pb.close();
                s(pb.finish());
                s(circle(0.5, 0.5, 0.14));
                fills.extend(circle(0.5, 0.5, 0.055));
            }
            Icon::Ear => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.32, 0.4);
                pb.cubic_to(0.3, 0.12, 0.72, 0.08, 0.72, 0.38);
                pb.cubic_to(0.72, 0.56, 0.56, 0.58, 0.56, 0.72);
                pb.cubic_to(0.56, 0.88, 0.38, 0.92, 0.34, 0.78);
                s(pb.finish());
                let mut pb = PathBuilder::new();
                pb.move_to(0.43, 0.42);
                pb.cubic_to(0.42, 0.28, 0.6, 0.26, 0.6, 0.4);
                pb.cubic_to(0.6, 0.48, 0.5, 0.5, 0.5, 0.58);
                s(pb.finish());
            }
            Icon::Hand => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.3, 0.56);
                pb.line_to(0.3, 0.24);
                pb.quad_to(0.36, 0.15, 0.42, 0.24);
                pb.line_to(0.42, 0.46);
                pb.line_to(0.42, 0.17);
                pb.quad_to(0.48, 0.08, 0.54, 0.17);
                pb.line_to(0.54, 0.46);
                pb.line_to(0.54, 0.21);
                pb.quad_to(0.6, 0.12, 0.66, 0.21);
                pb.line_to(0.66, 0.48);
                pb.line_to(0.66, 0.32);
                pb.quad_to(0.72, 0.24, 0.78, 0.32);
                pb.line_to(0.78, 0.62);
                pb.cubic_to(0.78, 0.82, 0.66, 0.9, 0.52, 0.9);
                pb.cubic_to(0.4, 0.9, 0.32, 0.84, 0.26, 0.74);
                pb.line_to(0.14, 0.55);
                pb.quad_to(0.19, 0.46, 0.27, 0.52);
                pb.line_to(0.3, 0.58);
                s(pb.finish());
            }
            Icon::Target => {
                s(circle(0.5, 0.5, 0.38));
                s(circle(0.5, 0.5, 0.23));
                fills.extend(circle(0.5, 0.5, 0.08));
            }
            Icon::Network => {
                let dots = [(0.5, 0.5), (0.5, 0.15), (0.17, 0.74), (0.83, 0.74)];
                let r = 0.09;
                for &(x, y) in &dots {
                    s(circle(x, y, r));
                }
                let mut join = |a: (f32, f32), b: (f32, f32)| {
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let d = (dx * dx + dy * dy).sqrt();
                    let (ux, uy) = (dx / d * r, dy / d * r);
                    s(poly(&[(a.0 + ux, a.1 + uy), (b.0 - ux, b.1 - uy)], false));
                };
                for k in 1..4 {
                    join(dots[0], dots[k]);
                }
                join(dots[2], dots[3]);
            }
            Icon::Book => {
                for m in [1., -1.] {
                    let x = |v: f32| 0.5 + m * (v - 0.5);
                    let mut pb = PathBuilder::new();
                    pb.move_to(0.5, 0.28);
                    pb.cubic_to(x(0.38), 0.2, x(0.22), 0.19, x(0.1), 0.24);
                    pb.line_to(x(0.1), 0.78);
                    pb.cubic_to(x(0.22), 0.73, x(0.38), 0.74, 0.5, 0.82);
                    s(pb.finish());
                }
                s(poly(&[(0.5, 0.28), (0.5, 0.82)], false));
            }
            Icon::Cap => {
                s(poly(
                    &[(0.5, 0.2), (0.92, 0.4), (0.5, 0.6), (0.08, 0.4)],
                    true,
                ));
                let mut pb = PathBuilder::new();
                pb.move_to(0.26, 0.5);
                pb.line_to(0.26, 0.7);
                pb.cubic_to(0.36, 0.82, 0.64, 0.82, 0.74, 0.7);
                pb.line_to(0.74, 0.5);
                s(pb.finish());
                s(poly(&[(0.92, 0.4), (0.92, 0.66)], false));
            }
            Icon::Leaf => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.18, 0.82);
                pb.cubic_to(0.14, 0.4, 0.44, 0.14, 0.86, 0.14);
                pb.cubic_to(0.86, 0.56, 0.6, 0.86, 0.18, 0.82);
                pb.close();
                s(pb.finish());
                s(poly(&[(0.18, 0.82), (0.62, 0.38)], false));
            }
            Icon::Sun => {
                s(circle(0.5, 0.5, 0.17));
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    let (c, si) = (a.cos(), a.sin());
                    s(poly(
                        &[
                            (0.5 + c * 0.29, 0.5 + si * 0.29),
                            (0.5 + c * 0.41, 0.5 + si * 0.41),
                        ],
                        false,
                    ));
                }
            }
            Icon::Moon => {
                // A crescent: the outer circle's left side, then a second
                // circle's back up through the same two tips.
                let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| {
                    (0..=16).map(move |k| {
                        let a = (from + (to - from) * k as f32 / 16.).to_radians();
                        (cx + r * a.cos(), cy + r * a.sin())
                    })
                };
                let mut pts: Vec<(f32, f32)> = arc(0.5, 0.5, 0.36, 300., 60.).collect();
                // The inner circle (centre 0.74, 0.5) passes through the
                // outer arc's lower tip, and by symmetry its upper one.
                let (tx, ty) = pts[pts.len() - 1];
                let (dx, dy) = (tx - 0.74, ty - 0.5);
                let tip = dy.atan2(dx).to_degrees();
                pts.extend(arc(0.74, 0.5, dx.hypot(dy), tip, 360. - tip).skip(1));
                s(poly(&pts, true));
            }
            Icon::Home => {
                s(poly(&[(0.1, 0.5), (0.5, 0.15), (0.9, 0.5)], false));
                s(poly(
                    &[(0.22, 0.4), (0.22, 0.86), (0.78, 0.86), (0.78, 0.4)],
                    false,
                ));
                s(poly(
                    &[(0.42, 0.86), (0.42, 0.64), (0.58, 0.64), (0.58, 0.86)],
                    false,
                ));
            }
            Icon::Music => {
                s(poly(
                    &[(0.38, 0.72), (0.38, 0.24), (0.8, 0.14), (0.8, 0.64)],
                    false,
                ));
                s(poly(&[(0.38, 0.35), (0.8, 0.25)], false));
                fills.extend(circle(0.29, 0.72, 0.1));
                fills.extend(circle(0.71, 0.64, 0.1));
            }
            Icon::Puzzle => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.18, 0.32);
                pb.line_to(0.38, 0.32);
                pb.cubic_to(0.3, 0.12, 0.62, 0.12, 0.54, 0.32);
                pb.line_to(0.76, 0.32);
                pb.line_to(0.76, 0.48);
                pb.cubic_to(0.94, 0.4, 0.94, 0.72, 0.76, 0.64);
                pb.line_to(0.76, 0.86);
                pb.line_to(0.18, 0.86);
                pb.close();
                s(pb.finish());
            }
            Icon::Rocket => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.08);
                pb.cubic_to(0.68, 0.22, 0.68, 0.5, 0.61, 0.66);
                pb.line_to(0.39, 0.66);
                pb.cubic_to(0.32, 0.5, 0.32, 0.22, 0.5, 0.08);
                pb.close();
                s(pb.finish());
                s(circle(0.5, 0.36, 0.07));
                s(poly(&[(0.36, 0.5), (0.24, 0.68), (0.39, 0.66)], false));
                s(poly(&[(0.64, 0.5), (0.76, 0.68), (0.61, 0.66)], false));
                s(poly(&[(0.44, 0.75), (0.5, 0.9), (0.56, 0.75)], false));
            }
            Icon::Trophy => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.3, 0.14);
                pb.line_to(0.7, 0.14);
                pb.line_to(0.7, 0.34);
                pb.cubic_to(0.7, 0.56, 0.3, 0.56, 0.3, 0.34);
                pb.close();
                s(pb.finish());
                for m in [1., -1.] {
                    let x = |v: f32| 0.5 + m * (v - 0.5);
                    let mut pb = PathBuilder::new();
                    pb.move_to(x(0.3), 0.2);
                    pb.cubic_to(x(0.12), 0.2, x(0.12), 0.44, x(0.33), 0.44);
                    s(pb.finish());
                }
                s(poly(&[(0.5, 0.5), (0.5, 0.7)], false));
                s(rect(0.33, 0.7, 0.67, 0.84, 0.3));
            }
            Icon::Apple => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.3);
                pb.cubic_to(0.36, 0.2, 0.14, 0.28, 0.16, 0.52);
                pb.cubic_to(0.18, 0.74, 0.34, 0.9, 0.5, 0.82);
                pb.cubic_to(0.66, 0.9, 0.82, 0.74, 0.84, 0.52);
                pb.cubic_to(0.86, 0.28, 0.64, 0.2, 0.5, 0.3);
                pb.close();
                s(pb.finish());
                s(poly(&[(0.5, 0.3), (0.53, 0.14)], false));
                let mut pb = PathBuilder::new();
                pb.move_to(0.53, 0.2);
                pb.quad_to(0.64, 0.07, 0.74, 0.13);
                pb.quad_to(0.66, 0.25, 0.53, 0.2);
                s(pb.finish());
            }
            Icon::Drop => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.1);
                pb.cubic_to(0.6, 0.3, 0.78, 0.44, 0.78, 0.6);
                pb.cubic_to(0.78, 0.78, 0.64, 0.88, 0.5, 0.88);
                pb.cubic_to(0.36, 0.88, 0.22, 0.78, 0.22, 0.6);
                pb.cubic_to(0.22, 0.44, 0.4, 0.3, 0.5, 0.1);
                pb.close();
                s(pb.finish());
            }
            Icon::Pin => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.9);
                pb.cubic_to(0.3, 0.64, 0.2, 0.52, 0.2, 0.38);
                pb.cubic_to(0.2, 0.2, 0.34, 0.1, 0.5, 0.1);
                pb.cubic_to(0.66, 0.1, 0.8, 0.2, 0.8, 0.38);
                pb.cubic_to(0.8, 0.52, 0.7, 0.64, 0.5, 0.9);
                pb.close();
                s(pb.finish());
                s(circle(0.5, 0.38, 0.1));
            }
            Icon::Shield => {
                let mut pb = PathBuilder::new();
                pb.move_to(0.5, 0.1);
                pb.line_to(0.82, 0.22);
                pb.line_to(0.82, 0.46);
                pb.cubic_to(0.82, 0.68, 0.66, 0.82, 0.5, 0.9);
                pb.cubic_to(0.34, 0.82, 0.18, 0.68, 0.18, 0.46);
                pb.line_to(0.18, 0.22);
                pb.close();
                s(pb.finish());
                s(poly(&[(0.36, 0.5), (0.46, 0.6), (0.64, 0.4)], false));
            }
        }
        (strokes, fills)
    }

    /// Draws the icon in `r` (a square), its lines `weight` px wide.
    pub fn draw(self, pix: &mut Pixmap, r: Rect, color: Rgb, alpha: f32, weight: f32) {
        self.draw_placed(pix, r, color, alpha, weight, Transform::identity());
    }

    /// `draw`, with `r` in a space of its own that `place` puts on the
    /// picture (an icon scaled or turned about a point).
    pub fn draw_placed(
        self,
        pix: &mut Pixmap,
        r: Rect,
        color: Rgb,
        alpha: f32,
        weight: f32,
        place: Transform,
    ) {
        if alpha <= 0.01 {
            return;
        }
        let size = r.width().min(r.height());
        let tf = Transform::from_row(size, 0., 0., size, r.left(), r.top()).post_concat(place);
        let (strokes, fills) = self.paths();
        let stroke = Stroke {
            width: weight / size,
            line_cap: LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        let p = paint(rgba(color, alpha));
        for path in &strokes {
            pix.stroke_path(path, &p, &stroke, tf, None);
        }
        for path in &fills {
            pix.fill_path(path, &p, FillRule::Winding, tf, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_icons_from_words() {
        assert_eq!(
            Icon::guess("Retriever", "semantic vector search", true),
            Icon::Search
        );
        assert_eq!(
            Icon::guess("Knowledge base", "vector store", true),
            Icon::Database
        );
        assert_eq!(Icon::guess("LLM", "generate the answer", true), Icon::Chip);
        assert_eq!(
            Icon::guess("Query", "What's our rate limit?", true),
            Icon::Chat
        );
        assert_eq!(Icon::guess("Maintain", "", false), Icon::Sparkle);
        assert_eq!(Icon::parse("Database"), Some(Icon::Database));
        // Memory is a database only where the topic is technical.
        assert_eq!(Icon::guess("Memory", "", true), Icon::Database);
        assert_ne!(Icon::guess("Memory", "", false), Icon::Database);
    }

    #[test]
    fn everyday_topics_get_everyday_icons() {
        let text = "Brain Memory Thinking Emotions";
        assert!(!technical(text));
        assert!(technical("Query the vector database through the API"));
        let row = |labels: &[&str]| {
            let items: Vec<(&str, &str, Option<Icon>)> =
                labels.iter().map(|l| (*l, "", None)).collect();
            Icon::guess_all(&items, false)
        };
        assert_eq!(
            row(&["Brain", "Memory", "Thinking", "Emotions"]),
            [Icon::Brain, Icon::Book, Icon::Bulb, Icon::Heart]
        );
        assert_eq!(
            row(&[
                "Receive Information",
                "Process Signals",
                "Make Decisions",
                "Take Action"
            ]),
            [Icon::Eye, Icon::Network, Icon::Target, Icon::Hand]
        );
        // A named icon stays, and isn't picked again for another.
        let items = [("Notes", "", Some(Icon::Brain)), ("Memory", "", None)];
        assert_eq!(Icon::guess_all(&items, false), [Icon::Brain, Icon::Book]);
    }

    #[test]
    fn every_icon_draws() {
        let mut pix = Pixmap::new(64, 64).unwrap();
        for (icon, _, _, _) in TABLE {
            pix.fill(tiny_skia::Color::TRANSPARENT);
            icon.draw(
                &mut pix,
                Rect::from_xywh(4., 4., 56., 56.).unwrap(),
                (255, 255, 255),
                1.,
                3.,
            );
            assert!(pix.pixels().iter().any(|p| p.alpha() > 0), "{icon:?}");
        }
    }
}
