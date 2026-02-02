use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tower_lsp_server::jsonrpc::Result as JsonResult;
use tower_lsp_server::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    Command, CompletionItem, CompletionItemKind, CompletionList, CompletionOptions, CompletionParams,
    CompletionResponse, CompletionTextEdit, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    ExecuteCommandOptions, ExecuteCommandParams, InitializeParams, InitializeResult,
    InitializedParams, MessageType, Position, Range, ServerCapabilities, ServerInfo, TextEdit,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, Uri, WorkspaceEdit,
};
use tower_lsp_server::{Client, LanguageServer};

use crate::dictionary::{Dictionary, SimpleDictionary};
use crate::pos_conv::LineIndex;

const CMD_ADD_TO_DICTIONARY: &str = "polski-ls.addToDictionary";

/// Document state stored for each open file.
struct DocumentState {
    source: Vec<char>,
    line_index: LineIndex,
}

/// LSP Backend implementation.
pub struct Backend {
    client: Client,
    documents: Mutex<HashMap<Uri, DocumentState>>,
    dictionary: Arc<Mutex<SimpleDictionary>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
            dictionary: Arc::new(Mutex::new(SimpleDictionary::with_user_extensions())),
        }
    }

    /// Generate completions for the given position.
    async fn generate_completions(
        &self,
        uri: &Uri,
        position: Position,
    ) -> JsonResult<Vec<CompletionItem>> {
        let documents = self.documents.lock().await;

        let Some(doc_state) = documents.get(uri) else {
            return Ok(Vec::new());
        };

        let source = &doc_state.source;
        let line_index = &doc_state.line_index;

        // Check for race condition (completion before didChange)
        if line_index.is_position_out_of_bounds(source, position) {
            return Ok(Vec::new());
        }

        // Convert position to character index
        let cursor_idx = line_index.position_to_index(source, position);

        // Find word start by scanning backward
        let mut word_start = cursor_idx;
        while word_start > 0 {
            let ch = source[word_start - 1];
            if !is_word_char(ch) {
                break;
            }
            word_start -= 1;
        }

        // Extract prefix
        let prefix: Vec<char> = source[word_start..cursor_idx].to_vec();

        let prefix_string: String = prefix.iter().collect();

        // For very short prefixes, just check if we should offer "add to dictionary"
        if prefix.len() < 2 {
            eprintln!("[POLSKI-LS] prefix too short: {} chars", prefix.len());
            return Ok(Vec::new());
        }

        eprintln!("[POLSKI-LS] looking up prefix: '{}'", prefix_string);

        // Get fuzzy matches from dictionary
        let max_edit_distance = if prefix.len() <= 3 { 1 } else { 2 };
        let dictionary = self.dictionary.lock().await;
        let fuzzy_matches = dictionary.fuzzy_match(&prefix, max_edit_distance, 200);
        drop(dictionary);

        // Score and sort matches
        let mut scored: Vec<(String, f32)> = fuzzy_matches
            .into_iter()
            .map(|m| {
                let word_str: String = m.word.iter().collect();
                let word = apply_capitalization(&prefix, &word_str);
                let score = calculate_completion_score(&prefix, &m.word, m.edit_distance, m.is_common);
                (word, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate word start position for text_edit
        let word_start_position = line_index.index_to_position(source, word_start);

        // Convert to CompletionItems
        let items: Vec<CompletionItem> = scored
            .into_iter()
            .take(50)
            .enumerate()
            .map(|(idx, (word, _score))| CompletionItem {
                label: word.clone(),
                kind: Some(CompletionItemKind::TEXT),
                detail: Some("Polish".to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: Range {
                        start: word_start_position,
                        end: position,
                    },
                    new_text: word,
                })),
                filter_text: Some(prefix_string.clone()),
                sort_text: Some(format!("{:05}", idx + 1)),
                ..Default::default()
            })
            .collect();

        Ok(items)
    }

    /// Check spelling and publish diagnostics for unknown words.
    async fn publish_diagnostics(&self, uri: &Uri, source: &[char], line_index: &LineIndex) {
        let words = extract_words(source);
        let mut diagnostics = Vec::new();

        for (word_chars, start_idx, end_idx) in words {
            // Skip short words (1-2 chars) - too many false positives
            if word_chars.len() < 3 {
                continue;
            }

            // Skip words that are all digits
            if word_chars.iter().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let dictionary = self.dictionary.lock().await;
            if !dictionary.contains(&word_chars) {
                let word: String = word_chars.iter().collect();
                let start_pos = line_index.index_to_position(source, start_idx);
                let end_pos = line_index.index_to_position(source, end_idx);

                diagnostics.push(Diagnostic {
                    range: Range {
                        start: start_pos,
                        end: end_pos,
                    },
                    severity: Some(DiagnosticSeverity::HINT),
                    source: Some("polski-ls".to_string()),
                    message: format!("Unknown word: '{}'", word),
                    ..Default::default()
                });
            }
        }

        eprintln!(
            "[POLSKI-LS] Publishing {} diagnostics for {:?}",
            diagnostics.len(),
            uri
        );
        self.client.publish_diagnostics(uri.clone(), diagnostics, None).await;
    }
}

/// Extract words from source text with their start and end indices.
fn extract_words(source: &[char]) -> Vec<(Vec<char>, usize, usize)> {
    let mut words = Vec::new();
    let mut i = 0;

    while i < source.len() {
        // Skip non-word characters
        if !is_word_char(source[i]) {
            i += 1;
            continue;
        }

        // Found start of a word
        let start = i;
        while i < source.len() && is_word_char(source[i]) {
            i += 1;
        }
        let end = i;

        let word: Vec<char> = source[start..end].to_vec();
        words.push((word, start, end));
    }

    words
}

/// Check if a character is part of a word (including Polish diacritics).
fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric()
        || matches!(
            ch,
            'ą' | 'ć' | 'ę' | 'ł' | 'ń' | 'ó' | 'ś' | 'ź' | 'ż'
                | 'Ą' | 'Ć' | 'Ę' | 'Ł' | 'Ń' | 'Ó' | 'Ś' | 'Ź' | 'Ż'
        )
}

/// Apply capitalization from original word to suggestion.
/// If original starts with uppercase, capitalize first letter of suggestion.
fn apply_capitalization(original: &[char], suggestion: &str) -> String {
    let starts_uppercase = original.first().map_or(false, |c| c.is_uppercase());
    if starts_uppercase {
        let mut chars: Vec<char> = suggestion.chars().collect();
        if let Some(first) = chars.first_mut() {
            *first = first.to_uppercase().next().unwrap_or(*first);
        }
        chars.into_iter().collect()
    } else {
        suggestion.to_string()
    }
}

/// Calculate completion score for ranking.
fn calculate_completion_score(
    query: &[char],
    candidate: &[char],
    edit_distance: u8,
    is_common: bool,
) -> f32 {
    let mut score = 100.0;

    // Edit distance penalty
    score -= match edit_distance {
        0 => 0.0,
        1 => 20.0,
        2 => 50.0,
        _ => 100.0,
    };

    // First letter match bonus
    if !query.is_empty() && !candidate.is_empty() {
        if query[0].eq_ignore_ascii_case(&candidate[0]) {
            score += 50.0;
        } else {
            score -= 30.0;
        }
    }

    // Prefix match bonus
    let prefix_match_len = query
        .iter()
        .zip(candidate.iter())
        .take_while(|(q, c)| q.eq_ignore_ascii_case(c))
        .count();
    score += (prefix_match_len as f32) * 8.0;

    // Common word bonus
    if is_common {
        score += 35.0;
    }

    score
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> JsonResult<InitializeResult> {
        eprintln!("[POLSKI-LS] initialize called");
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "polski-ls".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    // Trigger on all letters including Polish diacritics
                    trigger_characters: Some(
                        "aąbcćdeęfghijklłmnńoópqrsśtuvwxyzźżAĄBCĆDEĘFGHIJKLŁMNŃOÓPQRSŚTUVWXYZŹŻ"
                            .chars()
                            .map(String::from)
                            .collect(),
                    ),
                    // Space auto-accepts first suggestion for natural spell-checker flow
                    all_commit_characters: Some(vec![" ".to_string()]),
                    work_done_progress_options: Default::default(),
                    completion_item: None,
                }),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        will_save: None,
                        will_save_wait_until: None,
                        save: None,
                    },
                )),
                code_action_provider: Some(tower_lsp_server::lsp_types::CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![CMD_ADD_TO_DICTIONARY.to_string()],
                    work_done_progress_options: Default::default(),
                }),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        eprintln!("[POLSKI-LS] initialized - server ready!");
        self.client
            .log_message(MessageType::INFO, "polski-ls initialized!")
            .await;
    }

    async fn shutdown(&self) -> JsonResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        eprintln!("[POLSKI-LS] did_open: {:?}", params.text_document.uri);
        let source: Vec<char> = params.text_document.text.chars().collect();
        let line_index = LineIndex::new(&source);

        // Publish diagnostics before taking the lock to avoid holding it during async call
        self.publish_diagnostics(&params.text_document.uri, &source, &line_index)
            .await;

        let mut documents = self.documents.lock().await;
        documents.insert(
            params.text_document.uri,
            DocumentState { source, line_index },
        );
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        eprintln!("[POLSKI-LS] did_change: {:?}", params.text_document.uri);
        let Some(last) = params.content_changes.last() else {
            return;
        };

        let source: Vec<char> = last.text.chars().collect();
        let line_index = LineIndex::new(&source);

        // Publish diagnostics before taking the lock
        self.publish_diagnostics(&params.text_document.uri, &source, &line_index)
            .await;

        let mut documents = self.documents.lock().await;
        documents.insert(
            params.text_document.uri,
            DocumentState { source, line_index },
        );
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Clear diagnostics for closed document
        self.client
            .publish_diagnostics(params.text_document.uri.clone(), vec![], None)
            .await;

        let mut documents = self.documents.lock().await;
        documents.remove(&params.text_document.uri);
    }

    async fn completion(&self, params: CompletionParams) -> JsonResult<Option<CompletionResponse>> {
        let pos = params.text_document_position.position;
        eprintln!("[POLSKI-LS] completion: pos={}:{}", pos.line, pos.character);

        let items = self
            .generate_completions(
                &params.text_document_position.text_document.uri,
                params.text_document_position.position,
            )
            .await?;

        eprintln!("[POLSKI-LS] returning {} completions", items.len());
        if !items.is_empty() {
            let labels: Vec<_> = items.iter().take(5).map(|i| &i.label).collect();
            eprintln!("[POLSKI-LS] top 5: {:?}", labels);
        }

        if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CompletionResponse::List(CompletionList {
                is_incomplete: true,
                items,
            })))
        }
    }

    async fn code_action(&self, params: CodeActionParams) -> JsonResult<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let range = params.range;

        eprintln!(
            "[POLSKI-LS] code_action: {:?} at {}:{}-{}:{}",
            uri, range.start.line, range.start.character, range.end.line, range.end.character
        );

        let documents = self.documents.lock().await;
        let Some(doc_state) = documents.get(uri) else {
            return Ok(None);
        };

        let source = &doc_state.source;
        let line_index = &doc_state.line_index;

        // Find the word at the cursor position
        let start_idx = line_index.position_to_index(source, range.start);

        // Find word boundaries
        let mut word_start = start_idx;
        while word_start > 0 && is_word_char(source[word_start - 1]) {
            word_start -= 1;
        }

        let mut word_end = start_idx;
        while word_end < source.len() && is_word_char(source[word_end]) {
            word_end += 1;
        }

        if word_start == word_end {
            return Ok(None);
        }

        let word: Vec<char> = source[word_start..word_end].to_vec();
        let word_string: String = word.iter().collect();

        // Check if word is unknown
        let dictionary = self.dictionary.lock().await;
        if dictionary.contains(&word) {
            return Ok(None);
        }

        eprintln!("[POLSKI-LS] Generating suggestions for: '{}'", word_string);

        // Get fuzzy matches for suggestions
        let max_edit_distance = if word.len() <= 3 { 1 } else { 2 };
        let fuzzy_matches = dictionary.fuzzy_match(&word, max_edit_distance, 10);

        if fuzzy_matches.is_empty() {
            return Ok(None);
        }

        let word_range = Range {
            start: line_index.index_to_position(source, word_start),
            end: line_index.index_to_position(source, word_end),
        };

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        // Add "Add to dictionary" action first
        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Add '{}' to dictionary", word_string),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: None,
            edit: None,
            command: Some(Command {
                title: format!("Add '{}' to dictionary", word_string),
                command: CMD_ADD_TO_DICTIONARY.to_string(),
                arguments: Some(vec![serde_json::json!({
                    "word": word_string,
                    "uri": uri.to_string()
                })]),
            }),
            ..Default::default()
        }));

        for m in fuzzy_matches {
            let suggestion_str: String = m.word.iter().collect();
            let suggestion = apply_capitalization(&word, &suggestion_str);

            let mut changes = HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: word_range,
                    new_text: suggestion.clone(),
                }],
            );

            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Change to '{}'", suggestion),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }

        eprintln!("[POLSKI-LS] Returning {} code actions", actions.len());
        Ok(Some(actions))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> JsonResult<Option<serde_json::Value>> {
        eprintln!("[POLSKI-LS] execute_command: {}", params.command);

        if params.command == CMD_ADD_TO_DICTIONARY {
            if let Some(arg) = params.arguments.first() {
                if let (Some(word), Some(uri_str)) = (
                    arg.get("word").and_then(|v| v.as_str()),
                    arg.get("uri").and_then(|v| v.as_str()),
                ) {
                    eprintln!("[POLSKI-LS] Adding word to dictionary: '{}'", word);

                    // Add word to dictionary
                    let mut dictionary = self.dictionary.lock().await;
                    if let Err(e) = dictionary.add_user_word(word) {
                        eprintln!("[POLSKI-LS] Error adding word to dictionary: {}", e);
                        self.client
                            .show_message(MessageType::ERROR, format!("Failed to add word to dictionary: {}", e))
                            .await;
                        return Ok(None);
                    }
                    drop(dictionary);

                    // Show success message
                    self.client
                        .show_message(MessageType::INFO, format!("Added '{}' to dictionary", word))
                        .await;

                    // Refresh diagnostics for the document
                    if let Ok(uri) = uri_str.parse::<Uri>() {
                        let documents = self.documents.lock().await;
                        if let Some(doc_state) = documents.get(&uri) {
                            let source = doc_state.source.clone();
                            let line_index = doc_state.line_index.clone();
                            drop(documents);

                            self.publish_diagnostics(&uri, &source, &line_index).await;
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_word_char_ascii() {
        assert!(is_word_char('a'));
        assert!(is_word_char('Z'));
        assert!(is_word_char('5'));
        assert!(!is_word_char(' '));
        assert!(!is_word_char('.'));
        assert!(!is_word_char('\n'));
    }

    #[test]
    fn test_is_word_char_polish() {
        assert!(is_word_char('ą'));
        assert!(is_word_char('Ą'));
        assert!(is_word_char('ć'));
        assert!(is_word_char('ę'));
        assert!(is_word_char('ł'));
        assert!(is_word_char('ń'));
        assert!(is_word_char('ó'));
        assert!(is_word_char('ś'));
        assert!(is_word_char('ź'));
        assert!(is_word_char('ż'));
        assert!(is_word_char('Ż'));
    }

    #[test]
    fn test_apply_capitalization_lowercase() {
        let original: Vec<char> = "słodko".chars().collect();
        assert_eq!(apply_capitalization(&original, "słodki"), "słodki");
    }

    #[test]
    fn test_apply_capitalization_uppercase() {
        let original: Vec<char> = "Słodko".chars().collect();
        assert_eq!(apply_capitalization(&original, "słodki"), "Słodki");
    }

    #[test]
    fn test_apply_capitalization_polish_uppercase() {
        let original: Vec<char> = "Żółty".chars().collect();
        assert_eq!(apply_capitalization(&original, "żółw"), "Żółw");
    }

    #[test]
    fn test_apply_capitalization_empty() {
        let original: Vec<char> = Vec::new();
        assert_eq!(apply_capitalization(&original, "test"), "test");
    }

    #[test]
    fn test_extract_words() {
        let source: Vec<char> = "cześć świat".chars().collect();
        let words = extract_words(&source);
        assert_eq!(words.len(), 2);

        let word1: String = words[0].0.iter().collect();
        assert_eq!(word1, "cześć");
        assert_eq!(words[0].1, 0); // start
        assert_eq!(words[0].2, 5); // end

        let word2: String = words[1].0.iter().collect();
        assert_eq!(word2, "świat");
    }

    #[test]
    fn test_extract_words_with_punctuation() {
        let source: Vec<char> = "Dzień, dobry!".chars().collect();
        let words = extract_words(&source);
        assert_eq!(words.len(), 2);

        let word1: String = words[0].0.iter().collect();
        assert_eq!(word1, "Dzień");

        let word2: String = words[1].0.iter().collect();
        assert_eq!(word2, "dobry");
    }

    #[test]
    fn test_calculate_completion_score_exact_match() {
        let query: Vec<char> = "test".chars().collect();
        let candidate: Vec<char> = "test".chars().collect();
        let score = calculate_completion_score(&query, &candidate, 0, false);
        // 100 (base) + 50 (first letter) + 32 (4 chars prefix match * 8)
        assert_eq!(score, 182.0);
    }

    #[test]
    fn test_calculate_completion_score_common_word_bonus() {
        let query: Vec<char> = "test".chars().collect();
        let candidate: Vec<char> = "test".chars().collect();
        let score_common = calculate_completion_score(&query, &candidate, 0, true);
        let score_normal = calculate_completion_score(&query, &candidate, 0, false);
        assert_eq!(score_common - score_normal, 35.0);
    }

    #[test]
    fn test_calculate_completion_score_edit_distance_penalty() {
        let query: Vec<char> = "test".chars().collect();
        let candidate: Vec<char> = "tест".chars().collect();
        let score_0 = calculate_completion_score(&query, &candidate, 0, false);
        let score_1 = calculate_completion_score(&query, &candidate, 1, false);
        let score_2 = calculate_completion_score(&query, &candidate, 2, false);
        assert!(score_0 > score_1);
        assert!(score_1 > score_2);
    }
}
