//! A terminal's output on its way to the windows showing it: every window gets it; a window that
//! opens on a terminal another already shows first gets its recent output; with none showing it,
//! it waits for the first.
use std::collections::HashMap;

const PENDING_CAP: usize = 1 << 20;
const RECENT_CAP: usize = 256 << 10;

/// Where a window takes a terminal's output (a Tauri channel; a test's recorder).
pub trait Out {
    /// False once the window is gone.
    fn send(&self, text: String) -> bool;
}

pub struct Outputs<C: Out> {
    pending: HashMap<String, String>,
    recent: HashMap<String, String>,
    subs: HashMap<String, Vec<(String, C)>>,
}

impl<C: Out> Default for Outputs<C> {
    fn default() -> Self {
        Self { pending: HashMap::new(), recent: HashMap::new(), subs: HashMap::new() }
    }
}

/// Keeps the last `cap` bytes of `buf`, on a character boundary.
fn cap(buf: &mut String, cap: usize) {
    if buf.len() > cap {
        let from = buf.len() - cap;
        let cut = (from..buf.len()).find(|i| buf.is_char_boundary(*i)).unwrap_or(buf.len());
        buf.drain(..cut);
    }
}

impl<C: Out> Outputs<C> {
    /// `window` shows terminal `id` from now on (in place of what it had for it before).
    pub fn attach(&mut self, id: &str, window: &str, ch: C) {
        let subs = self.subs.entry(id.to_string()).or_default();
        subs.retain(|(w, _)| w != window);
        if let Some(buf) = self.pending.remove(id) {
            ch.send(buf);
        } else if let Some(recent) = self.recent.get(id).filter(|_| !subs.is_empty()) {
            // what the others have shown lately, from a line's start (not mid escape sequence)
            let start = recent.find('\n').map_or(recent.len(), |i| i + 1);
            if start < recent.len() {
                ch.send(recent[start..].to_string());
            }
        }
        subs.push((window.to_string(), ch));
    }

    pub fn output(&mut self, id: &str, text: String) {
        let recent = self.recent.entry(id.to_string()).or_default();
        recent.push_str(&text);
        cap(recent, RECENT_CAP);
        let subs = self.subs.entry(id.to_string()).or_default();
        subs.retain(|(_, ch)| ch.send(text.clone()));
        if subs.is_empty() {
            let buf = self.pending.entry(id.to_string()).or_default();
            buf.push_str(&text);
            cap(buf, PENDING_CAP);
        }
    }

    /// The terminal is gone.
    pub fn detach(&mut self, id: &str) {
        self.pending.remove(id);
        self.recent.remove(id);
        self.subs.remove(id);
    }

    /// The window is gone: it shows nothing any more.
    pub fn drop_window(&mut self, window: &str) {
        for subs in self.subs.values_mut() {
            subs.retain(|(w, _)| w != window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct Rec(Rc<RefCell<String>>, Rc<RefCell<bool>>);
    impl Out for Rec {
        fn send(&self, text: String) -> bool {
            if *self.1.borrow() {
                return false; // closed
            }
            self.0.borrow_mut().push_str(&text);
            true
        }
    }
    impl Rec {
        fn got(&self) -> String {
            self.0.borrow().clone()
        }
    }

    #[test]
    fn every_window_showing_a_terminal_gets_its_output() {
        let mut o = Outputs::default();
        o.output("t", "replay\n".into()); // before any window: kept for the first
        let main = Rec::default();
        o.attach("t", "main", main.clone());
        assert_eq!(main.got(), "replay\n");
        o.output("t", "a\n".into());
        // a second window starts from recent output, at a line's start
        let other = Rec::default();
        o.attach("t", "s-t", other.clone());
        assert_eq!(other.got(), "a\n", "the first (cut) line left out, then whole lines");
        o.output("t", "b".into());
        assert_eq!((main.got(), other.got()), ("replay\na\nb".into(), "a\nb".into()));
        // the second window closes: the first still gets it all
        o.drop_window("s-t");
        o.output("t", "c".into());
        assert_eq!((main.got(), other.got()), ("replay\na\nbc".into(), "a\nb".into()));
        // none left (a channel that fails is gone): kept for the next
        *main.1.borrow_mut() = true;
        o.output("t", "d".into());
        let again = Rec::default();
        o.attach("t", "main", again.clone());
        assert_eq!(again.got(), "d");
    }

    #[test]
    fn a_window_attaching_again_replaces_its_old_channel() {
        let mut o = Outputs::default();
        let (old, new) = (Rec::default(), Rec::default());
        o.attach("t", "main", old.clone());
        o.attach("t", "main", new.clone());
        o.output("t", "x".into());
        assert_eq!((old.got(), new.got()), (String::new(), "x".into()));
        o.detach("t");
        o.output("t", "y".into());
        assert_eq!(new.got(), "x", "a closed terminal sends nothing");
    }
}
