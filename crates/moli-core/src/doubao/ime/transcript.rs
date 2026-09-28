//! The text of a session, put together from the segments the service
//! reports.

/// Results whose start times are this close belong to the same segment;
/// the service moves a segment's start by up to half a second as it goes.
const SEGMENT_START_SLACK: f64 = 1.5;

/// The text of a session, put together from its segments.
#[derive(Default)]
pub(super) struct Transcript {
    /// In order of index, then start time.
    segments: Vec<Segment>,
    /// Added to indices, so that a follow-up session continues after the
    /// segments of the one before.
    base: i64,
}

struct Segment {
    index: i64,
    /// Seconds into the session, as first reported.
    start: Option<f64>,
    text: String,
    /// Being heard again in a new session; shown until that has results.
    stale: bool,
}

impl Segment {
    fn is(&self, index: i64, start: Option<f64>) -> bool {
        !self.stale
            && self.index == index
            && match (self.start, start) {
                (Some(a), Some(b)) => (a - b).abs() < SEGMENT_START_SLACK,
                _ => true,
            }
    }
}

impl Transcript {
    /// Takes in a result frame's payload; the full text if it changed.
    pub fn update(&mut self, payload: &str) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(payload).ok()?;
        let mut changed = false;
        for result in value.get("results")?.as_array()? {
            let Some(text) = result
                .get("text")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
            else {
                continue;
            };
            let index = self.base + result.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
            let start = result.get("start_time").and_then(|t| t.as_f64());
            self.segments.retain(|s| !s.stale);
            match self.segments.iter_mut().find(|s| s.is(index, start)) {
                Some(segment) if segment.text == text => {}
                Some(segment) => {
                    segment.text = text.to_string();
                    changed = true;
                }
                None => {
                    let at = self.segments.partition_point(|s| {
                        (s.index, s.start.unwrap_or(0.0)) <= (index, start.unwrap_or(0.0))
                    });
                    self.segments.insert(
                        at,
                        Segment {
                            index,
                            start,
                            text: text.to_string(),
                            stale: false,
                        },
                    );
                    changed = true;
                }
            }
        }
        changed.then(|| self.text())
    }

    /// The session broke off: its last segment may be unfinished, so a new
    /// session is to hear it again. The second that segment starts at in
    /// this session (0 to hear the whole session again).
    pub fn rewind(&mut self) -> f64 {
        let current = |s: &Segment| s.index >= self.base && !s.stale;
        let from = match self.segments.iter().rposition(current) {
            Some(last) if self.segments[last].start.is_some() => {
                self.segments[last].stale = true;
                self.segments[last].start.unwrap_or(0.0)
            }
            _ => {
                for s in self.segments.iter_mut().filter(|s| s.index >= self.base) {
                    s.stale = true;
                }
                0.0
            }
        };
        self.next_session();
        from
    }

    /// Later results belong to a new session, whose indices start over.
    pub fn next_session(&mut self) {
        self.base = self.segments.iter().map(|s| s.index + 1).max().unwrap_or(0);
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            let segment = segment.text.as_str();
            // Words of Latin script need a space between segments.
            if out.ends_with(|c: char| c.is_ascii_alphanumeric())
                && segment.starts_with(|c: char| c.is_ascii_alphanumeric())
            {
                out.push(' ');
            }
            out.push_str(segment);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(index: Option<i64>, text: &str) -> String {
        let mut r = serde_json::json!({"text": text, "is_interim": true});
        if let Some(i) = index {
            r["index"] = i.into();
        }
        serde_json::json!({"results": [r]}).to_string()
    }

    #[test]
    fn segments_add_up() {
        let mut t = Transcript::default();
        assert_eq!(t.update(&result(Some(0), "你好")).as_deref(), Some("你好"));
        assert_eq!(
            t.update(&result(Some(0), "你好世界。")).as_deref(),
            Some("你好世界。")
        );
        // After a pause the next segment starts from scratch.
        assert_eq!(
            t.update(&result(Some(1), "今天")).as_deref(),
            Some("你好世界。今天")
        );
        assert_eq!(t.update(&result(Some(1), "今天")), None);
        assert_eq!(
            t.update(&result(Some(1), "今天天气不错。")).as_deref(),
            Some("你好世界。今天天气不错。")
        );
    }

    fn timed(index: i64, start: f64, text: &str) -> String {
        serde_json::json!({"results": [
            {"text": text, "index": index, "start_time": start, "is_interim": true}
        ]})
        .to_string()
    }

    // As seen on 2026-09-28: 24 s into speech without a pause, the service
    // sends an empty result, then a new segment with the same index.
    #[test]
    fn a_long_segment_is_split_by_start_time() {
        let mut t = Transcript::default();
        t.update(&timed(0, 0.0, "一二三"));
        t.update(&timed(0, 0.0, "一二三四五。"));
        t.update(&timed(0, 0.0, ""));
        assert_eq!(
            t.update(&timed(0, 23.194, "六")).as_deref(),
            Some("一二三四五。六")
        );
        assert_eq!(
            t.update(&timed(0, 23.61, "六七八")).as_deref(),
            Some("一二三四五。六七八")
        );
        assert_eq!(
            t.update(&timed(0, 23.359, "六七八九。")).as_deref(),
            Some("一二三四五。六七八九。")
        );
        // A late word on the first segment stays in its place.
        assert_eq!(
            t.update(&timed(0, 0.2, "一二三四五，")).as_deref(),
            Some("一二三四五，六七八九。")
        );
    }

    #[test]
    fn a_rewind_hears_the_last_segment_again() {
        let mut t = Transcript::default();
        t.update(&timed(0, 0.0, "一二。"));
        t.update(&timed(1, 3.0, "三四"));
        assert_eq!(t.rewind(), 3.0);
        // The old text stays until the new session has some.
        assert_eq!(t.text(), "一二。三四");
        assert_eq!(
            t.update(&timed(0, 0.3, "三四五")).as_deref(),
            Some("一二。三四五")
        );
        assert_eq!(
            t.update(&timed(0, 0.3, "三四五六。")).as_deref(),
            Some("一二。三四五六。")
        );
    }

    #[test]
    fn a_rewind_without_times_hears_the_whole_session_again() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "一。"));
        t.next_session();
        t.update(&result(Some(0), "二"));
        assert_eq!(t.rewind(), 0.0);
        assert_eq!(
            t.update(&result(Some(0), "二三")).as_deref(),
            Some("一。二三")
        );
    }

    #[test]
    fn no_index_means_the_first_segment() {
        let mut t = Transcript::default();
        t.update(&result(None, "你好"));
        assert_eq!(
            t.update(&result(None, "你好世界")).as_deref(),
            Some("你好世界")
        );
    }

    #[test]
    fn empty_and_foreign_payloads_change_nothing() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "你好"));
        assert_eq!(t.update(&result(Some(0), "")), None);
        assert_eq!(t.update("{}"), None);
        assert_eq!(t.update("not json"), None);
        assert_eq!(t.text(), "你好");
    }

    #[test]
    fn latin_segments_are_spaced() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "hello"));
        t.update(&result(Some(1), "world"));
        t.update(&result(Some(2), "你好"));
        assert_eq!(t.text(), "hello world你好");
    }

    #[test]
    fn a_follow_up_session_continues_the_text() {
        let mut t = Transcript::default();
        t.update(&result(Some(0), "一。"));
        t.update(&result(Some(1), "二。"));
        t.next_session();
        assert_eq!(
            t.update(&result(Some(0), "三。")).as_deref(),
            Some("一。二。三。")
        );
    }
}
