/// Result of a fuzzy match operation.
#[derive(Debug, Clone)]
pub struct FuzzyMatchResult {
    pub word: Vec<char>,
    pub edit_distance: u8,
    pub is_common: bool,
}

/// Trait for dictionary implementations that support fuzzy matching.
pub trait Dictionary: Send + Sync {
    /// Check if a word exists in the dictionary (case-insensitive).
    fn contains(&self, word: &[char]) -> bool;

    /// Find words matching the prefix within the given edit distance.
    fn fuzzy_match(
        &self,
        prefix: &[char],
        max_edit_distance: u8,
        max_results: usize,
    ) -> Vec<FuzzyMatchResult>;
}

/// Simple in-memory dictionary implementation.
pub struct SimpleDictionary {
    words: Vec<(Vec<char>, bool)>, // (word, is_common)
    user_dict_path: Option<std::path::PathBuf>,
}

impl SimpleDictionary {
    /// Create a new empty dictionary.
    pub fn new() -> Self {
        Self {
            words: Vec::new(),
            user_dict_path: None,
        }
    }

    /// Add a word to the dictionary.
    pub fn add_word(&mut self, word: &str, is_common: bool) {
        self.words.push((word.chars().collect(), is_common));
    }

    /// Parse words from text content (one word per line, *prefix = common)
    fn parse_word_list(&mut self, content: &str) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue; // skip empty lines and comments
            }
            if let Some(word) = trimmed.strip_prefix('*') {
                self.add_word(word, true);
            } else {
                self.add_word(trimmed, false);
            }
        }
    }

    /// Load embedded baseline dictionary
    pub fn embedded() -> Self {
        let mut dict = Self::new();
        dict.parse_word_list(include_str!("../slowa.txt"));
        dict
    }

    /// Add a word to the in-memory dictionary and save it to the user dictionary file.
    pub fn add_user_word(&mut self, word: &str) -> std::io::Result<()> {
        // Add to in-memory dictionary
        let word_chars: Vec<char> = word.chars().collect();

        // Check if word already exists (case-insensitive)
        if self.contains(&word_chars) {
            eprintln!("[POLSKI-LS] Word '{}' already in dictionary", word);
            return Ok(());
        }

        self.words.push((word_chars, false));

        // Save to user dictionary file if path is set
        if let Some(path) = &self.user_dict_path {
            use std::io::Write;

            eprintln!("[POLSKI-LS] Saving word '{}' to {:?}", word, path);

            // Ensure parent directory exists
            if let Some(parent) = path.parent() {
                eprintln!("[POLSKI-LS] Creating directory: {:?}", parent);
                std::fs::create_dir_all(parent)?;
                eprintln!("[POLSKI-LS] Directory created successfully");
            }

            // Append word to file
            eprintln!("[POLSKI-LS] Opening file for append: {:?}", path);
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;

            writeln!(file, "{}", word)?;
            eprintln!("[POLSKI-LS] Successfully added '{}' to user dictionary: {:?}", word, path);
        } else {
            eprintln!("[POLSKI-LS] ERROR: No user_dict_path set, word '{}' not saved to file", word);
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "User dictionary path not configured. Config directory could not be determined."
            ));
        }

        Ok(())
    }

    /// Load embedded + user extension files from ~/.config/polski-ls/*.txt
    pub fn with_user_extensions() -> Self {
        let mut dict = Self::embedded();

        // Try to get config directory, fallback to $HOME/.config if not available
        let config_dir = dirs::config_dir().or_else(|| {
            eprintln!("[POLSKI-LS] dirs::config_dir() returned None, using fallback");
            std::env::var("HOME").ok().map(|home| {
                let path = std::path::PathBuf::from(home).join(".config");
                eprintln!("[POLSKI-LS] Fallback config dir: {:?}", path);
                path
            })
        });

        if let Some(config_dir) = config_dir {
            eprintln!("[POLSKI-LS] Config directory: {:?}", config_dir);
            let polski_ls_dir = config_dir.join("polski-ls");

            // Set the user dictionary path
            dict.user_dict_path = Some(polski_ls_dir.join("slownik.txt"));
            eprintln!("[POLSKI-LS] User dictionary path: {:?}", dict.user_dict_path);

            // Create the directory immediately to ensure it exists
            if !polski_ls_dir.exists() {
                eprintln!("[POLSKI-LS] Creating polski-ls directory: {:?}", polski_ls_dir);
                if let Err(e) = std::fs::create_dir_all(&polski_ls_dir) {
                    eprintln!("[POLSKI-LS] ERROR: Failed to create directory: {}", e);
                } else {
                    eprintln!("[POLSKI-LS] Successfully created directory");
                }
            }

            if polski_ls_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&polski_ls_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().map_or(false, |e| e == "txt") {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                eprintln!("[POLSKI-LS] Loading user dict: {:?}", path);
                                dict.parse_word_list(&content);
                            }
                        }
                    }
                }
            }
        } else {
            eprintln!("[POLSKI-LS] ERROR: Could not determine config directory!");
        }

        dict
    }
}

impl Default for SimpleDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl Dictionary for SimpleDictionary {
    fn contains(&self, word: &[char]) -> bool {
        self.words.iter().any(|(dict_word, _)| {
            dict_word.len() == word.len()
                && dict_word
                    .iter()
                    .zip(word.iter())
                    .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
        })
    }

    fn fuzzy_match(
        &self,
        prefix: &[char],
        max_edit_distance: u8,
        max_results: usize,
    ) -> Vec<FuzzyMatchResult> {
        let mut results: Vec<FuzzyMatchResult> = self
            .words
            .iter()
            .filter_map(|(word, is_common)| {
                let distance = levenshtein_distance(prefix, word);
                if distance <= max_edit_distance {
                    Some(FuzzyMatchResult {
                        word: word.clone(),
                        edit_distance: distance,
                        is_common: *is_common,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by edit distance first, then by common status
        results.sort_by(|a, b| {
            a.edit_distance
                .cmp(&b.edit_distance)
                .then_with(|| b.is_common.cmp(&a.is_common))
        });

        results.truncate(max_results);
        results
    }
}

/// Calculate the Levenshtein edit distance between two character sequences.
pub fn levenshtein_distance(a: &[char], b: &[char]) -> u8 {
    let m = a.len();
    let n = b.len();

    // Early termination for empty strings
    if m == 0 {
        return n.min(255) as u8;
    }
    if n == 0 {
        return m.min(255) as u8;
    }

    // Use two rows instead of full matrix to save memory
    let mut prev_row: Vec<usize> = (0..=n).collect();
    let mut curr_row: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr_row[0] = i;

        for j in 1..=n {
            let cost = if a[i - 1].eq_ignore_ascii_case(&b[j - 1]) {
                0
            } else {
                1
            };

            curr_row[j] = (prev_row[j] + 1) // deletion
                .min(curr_row[j - 1] + 1) // insertion
                .min(prev_row[j - 1] + cost); // substitution
        }

        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[n].min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_same() {
        let a: Vec<char> = "hello".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(levenshtein_distance(&a, &b), 0);
    }

    #[test]
    fn test_levenshtein_one_char_diff() {
        let a: Vec<char> = "hello".chars().collect();
        let b: Vec<char> = "hallo".chars().collect();
        assert_eq!(levenshtein_distance(&a, &b), 1);
    }

    #[test]
    fn test_levenshtein_insertion() {
        let a: Vec<char> = "helo".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(levenshtein_distance(&a, &b), 1);
    }

    #[test]
    fn test_levenshtein_deletion() {
        let a: Vec<char> = "hello".chars().collect();
        let b: Vec<char> = "helo".chars().collect();
        assert_eq!(levenshtein_distance(&a, &b), 1);
    }

    #[test]
    fn test_levenshtein_empty() {
        let a: Vec<char> = "".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(levenshtein_distance(&a, &b), 5);
    }

    #[test]
    fn test_fuzzy_match() {
        let mut dict = SimpleDictionary::new();
        dict.add_word("dzień", true);
        dict.add_word("dziecko", false);
        dict.add_word("dzisiaj", false);

        let prefix: Vec<char> = "dzie".chars().collect();
        let results = dict.fuzzy_match(&prefix, 2, 10);

        assert!(!results.is_empty());
        // "dzień" should match with edit distance 1
        assert!(results.iter().any(|r| {
            let word: String = r.word.iter().collect();
            word == "dzień" && r.edit_distance == 1
        }));
    }

    #[test]
    fn test_case_insensitive() {
        let a: Vec<char> = "Hello".chars().collect();
        let b: Vec<char> = "hello".chars().collect();
        assert_eq!(levenshtein_distance(&a, &b), 0);
    }

    #[test]
    fn test_embedded_dictionary() {
        let dict = SimpleDictionary::embedded();
        // Should have words from slowa.txt
        let prefix: Vec<char> = "dzień".chars().collect();
        let results = dict.fuzzy_match(&prefix, 0, 10);
        assert!(!results.is_empty());
        let word: String = results[0].word.iter().collect();
        assert_eq!(word, "dzień");
        assert!(results[0].is_common); // marked with * in slowa.txt
    }

    #[test]
    fn test_contains() {
        let dict = SimpleDictionary::embedded();
        // Common word should be found
        let word: Vec<char> = "dzień".chars().collect();
        assert!(dict.contains(&word));

        // Case-insensitive
        let word_upper: Vec<char> = "DZIEŃ".chars().collect();
        assert!(dict.contains(&word_upper));

        // Non-existent word
        let unknown: Vec<char> = "xyz123".chars().collect();
        assert!(!dict.contains(&unknown));
    }
}
