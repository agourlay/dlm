use rand::seq::IndexedRandom;

const USER_AGENTS: [&str; 16] = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.3537.92",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:144.0) Gecko/20100101 Firefox/144.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.3537.92",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 15.7; rv:144.0) Gecko/20100101 Firefox/144.0",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64; rv:144.0) Gecko/20100101 Firefox/144.0",
    "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:144.0) Gecko/20100101 Firefox/144.0",
    "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.7390.123 Mobile Safari/537.36",
    "Mozilla/5.0 (Android 16; Mobile; rv:144.0) Gecko/144.0 Firefox/144.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/605.1.15",
];

#[derive(Debug)]
pub enum UserAgent {
    CustomUserAgent(String),
    RandomUserAgent,
}

impl UserAgent {
    /// The header value to send. A random agent is drawn once here, so every
    /// request of the run carries the same one.
    pub fn into_header_value(self) -> String {
        match self {
            Self::CustomUserAgent(ua) => ua,
            Self::RandomUserAgent => random_user_agent().to_string(),
        }
    }
}

pub fn random_user_agent() -> &'static str {
    // safe unwrap because the array is not empty
    USER_AGENTS.choose(&mut rand::rng()).unwrap()
}

/// Print the built-in user-agent pool to stdout, one per numbered line.
/// Useful so users can see what `--random-user-agent` rotates through, or
/// pick a value to pass to `--user-agent`.
pub fn print_user_agents() {
    for (i, ua) in USER_AGENTS.iter().enumerate() {
        println!("{:>2}  {ua}", i + 1);
    }
}

#[cfg(test)]
mod user_agent_tests {
    use super::*;

    #[test]
    fn get_random_user_agent() {
        let ua = random_user_agent();
        assert!(ua.starts_with("Mozilla/5.0"));
    }
}
