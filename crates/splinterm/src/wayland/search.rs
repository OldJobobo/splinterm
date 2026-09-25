//! Client-local scrollback search lifecycle, shared by active and background panes.

use splinterm_protocol::{SearchMatch, SearchPage};

use super::{BoundedTextEditor, new_search_editor};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SearchStatus {
    #[default]
    Idle,
    Searching,
    Complete,
    Partial,
    Expired,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SearchUiState {
    pub input: Option<BoundedTextEditor>,
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub selected: usize,
    pub next_cursor: Option<String>,
    pub pending_reveal: Option<SearchMatch>,
    pub status: SearchStatus,
    pub generation: u64,
    pub pending_request: Option<u64>,
}

impl SearchUiState {
    pub fn close(&mut self) {
        // Never reuse a request identity when closing and reopening this pane's search.
        *self = Self {
            generation: self.generation,
            ..Self::default()
        };
    }

    pub fn open(&mut self) {
        self.close();
        self.input = Some(new_search_editor());
    }

    fn clear_results(&mut self) {
        self.matches.clear();
        self.selected = 0;
        self.next_cursor = None;
        self.pending_reveal = None;
    }

    /// A new submission replaces old results immediately; a page request is single-flight.
    pub fn begin(&mut self, query: String, next_page: bool) -> Option<u64> {
        if self.input.is_none() || (next_page && self.pending_request.is_some()) {
            return None;
        }
        self.clear_results();
        self.pending_request = None;
        self.query = query;
        if self.query.is_empty() {
            self.status = SearchStatus::Idle;
            return None;
        }
        self.generation = self.generation.checked_add(1).expect("search ID exhausted");
        self.pending_request = Some(self.generation);
        self.status = SearchStatus::Searching;
        Some(self.generation)
    }

    pub fn results(&mut self, request_id: u64, page: SearchPage) -> bool {
        if self.pending_request != Some(request_id) {
            return false;
        }
        self.pending_request = None;
        self.status = if page.timed_out {
            SearchStatus::Partial
        } else {
            SearchStatus::Complete
        };
        self.matches = page.matches;
        self.selected = 0;
        self.next_cursor = page.next_cursor;
        self.pending_reveal = self.matches.first().cloned();
        true
    }

    pub fn expired(&mut self, request_id: u64) -> bool {
        if self.pending_request != Some(request_id) {
            return false;
        }
        self.pending_request = None;
        self.clear_results();
        self.status = SearchStatus::Expired;
        true
    }

    pub fn status_text(&self) -> String {
        if self
            .input
            .as_ref()
            .is_none_or(|input| input.text() != self.query)
        {
            return "Enter to search · Esc close".to_owned();
        }
        match self.status {
            SearchStatus::Idle => "Enter to search · Esc close".to_owned(),
            SearchStatus::Searching => "Searching… · Esc close".to_owned(),
            SearchStatus::Expired => "Search expired · Enter to retry · Esc close".to_owned(),
            SearchStatus::Complete | SearchStatus::Partial => {
                let partial = if self.status == SearchStatus::Partial {
                    " · partial (time limit)"
                } else {
                    ""
                };
                let more = if self.next_cursor.is_some() {
                    " · more available"
                } else {
                    ""
                };
                format!(
                    "{}/{} matches{partial}{more} · Ctrl+N/P next/prev · Esc close",
                    self.selected.saturating_add(1).min(self.matches.len()),
                    self.matches.len(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use splinterm_core::SplintId;

    use super::*;

    fn page(timed_out: bool) -> SearchPage {
        SearchPage {
            splint_id: SplintId::new(),
            incarnation: 1,
            terminal_revision: 1,
            history_generation: 1,
            matches: vec![SearchMatch {
                row_id: 1,
                start_column: 0,
                end_column: 3,
                preview: "hit".into(),
            }],
            next_cursor: Some("next".into()),
            timed_out,
        }
    }

    fn submit(state: &mut SearchUiState, query: &str) -> u64 {
        state.input = Some(BoundedTextEditor::new(query.into(), 128, 128, false));
        state.begin(query.into(), false).unwrap()
    }

    #[test]
    fn search_cancel_and_reopen_never_reuses_request_identity() {
        let mut state = SearchUiState::default();
        state.open();
        let old = submit(&mut state, "old");
        state.close();
        assert!(!state.results(old, page(false)));
        assert!(!state.expired(old));
        assert!(state.pending_reveal.is_none());
        state.open();
        let new = submit(&mut state, "new");
        assert_ne!(old, new);
        assert!(!state.results(old, page(false)));
        assert!(!state.expired(old));
        assert_eq!(state.status, SearchStatus::Searching);
        assert!(state.results(new, page(false)));
        assert!(
            !state.results(new, page(true)),
            "duplicate reply must be ignored"
        );
        assert_eq!(state.status, SearchStatus::Complete);
    }

    #[test]
    fn search_replacement_rejects_old_success_and_expiry() {
        let mut state = SearchUiState::default();
        let old = submit(&mut state, "old");
        let new = submit(&mut state, "new");
        assert!(!state.results(old, page(false)));
        assert!(!state.expired(old));
        assert_eq!(state.pending_request, Some(new));
        assert!(state.matches.is_empty());
        assert!(state.results(new, page(false)));
        assert!(!state.expired(old));
        assert_eq!(state.matches.len(), 1);
    }

    #[test]
    fn search_new_query_clears_old_results_cursor_and_reveal() {
        let mut state = SearchUiState::default();
        let old = submit(&mut state, "old");
        assert!(state.results(old, page(false)));
        assert!(state.pending_reveal.is_some());
        submit(&mut state, "new");
        assert!(state.matches.is_empty());
        assert!(state.next_cursor.is_none());
        assert!(state.pending_reveal.is_none());
        assert_eq!(state.selected, 0);
        assert_eq!(state.status_text(), "Searching… · Esc close");
    }

    #[test]
    fn search_empty_submission_invalidates_in_flight_work() {
        let mut state = SearchUiState::default();
        let old = submit(&mut state, "old");
        state.input = Some(new_search_editor());
        assert_eq!(state.begin(String::new(), false), None);
        assert!(!state.results(old, page(false)));
        assert!(!state.expired(old));
        assert_eq!(state.status, SearchStatus::Idle);
        assert!(state.status_text().starts_with("Enter to search"));
    }

    #[test]
    fn search_pages_are_single_flight_and_superseded_by_new_submission() {
        let mut state = SearchUiState::default();
        let first = submit(&mut state, "query");
        assert!(state.results(first, page(false)));
        let next = state.begin("query".into(), true).unwrap();
        assert_ne!(first, next);
        assert_eq!(state.begin("query".into(), true), None);
        assert_eq!(state.pending_request, Some(next));
        let replacement = submit(&mut state, "replacement");
        assert!(!state.results(next, page(false)));
        assert!(!state.expired(next));
        assert!(state.results(replacement, page(true)));
        assert_eq!(state.status, SearchStatus::Partial);
    }

    #[test]
    fn search_status_distinguishes_partial_empty_complete_expired_and_draft() {
        let mut state = SearchUiState::default();
        let first = submit(&mut state, "query");
        assert!(state.results(first, page(true)));
        assert!(
            state
                .status_text()
                .contains("1/1 matches · partial (time limit) · more available")
        );
        let next = state.begin("query".into(), true).unwrap();
        let mut empty = page(true);
        empty.matches.clear();
        empty.next_cursor = None;
        assert!(state.results(next, empty.clone()));
        assert!(
            state
                .status_text()
                .contains("0/0 matches · partial (time limit)")
        );
        assert!(!state.status_text().contains("more available"));
        let complete = submit(&mut state, "query");
        empty.timed_out = false;
        assert!(state.results(complete, empty));
        assert!(state.status_text().starts_with("0/0 matches · Ctrl+N/P"));
        let expired = submit(&mut state, "query");
        assert!(state.expired(expired));
        assert!(
            state
                .status_text()
                .starts_with("Search expired · Enter to retry")
        );
        assert!(state.pending_reveal.is_none());
        assert!(state.next_cursor.is_none());
        state.input.as_mut().unwrap().insert(" edited");
        assert!(state.status_text().starts_with("Enter to search"));
        assert!(!state.status_text().contains("matches"));
    }

    #[test]
    fn search_reopening_clears_completed_results_and_pending_reveal() {
        let mut state = SearchUiState::default();
        let request = submit(&mut state, "query");
        assert!(state.results(request, page(false)));
        assert!(state.status_text().contains("more available"));
        state.open();
        assert!(state.query.is_empty());
        assert!(state.matches.is_empty());
        assert!(state.next_cursor.is_none());
        assert!(state.pending_reveal.is_none());
        assert_eq!(state.status, SearchStatus::Idle);
        assert!(state.status_text().starts_with("Enter to search"));
    }
}
