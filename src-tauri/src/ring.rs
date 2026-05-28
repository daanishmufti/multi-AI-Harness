//! A byte-bounded ring buffer holding the most recent terminal output of a
//! session. This is what makes "lazy rendering" cheap: detached sessions keep
//! filling a small capped buffer instead of streaming to the UI, and on attach
//! we replay the buffer once so the terminal repaints instantly.

use std::collections::VecDeque;

pub struct RingBuffer {
    buf: VecDeque<u8>,
    cap: usize,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap.min(64 * 1024)),
            cap: cap.max(4096),
        }
    }

    /// Append bytes, evicting the oldest data past the capacity.
    pub fn push(&mut self, data: &[u8]) {
        if data.len() >= self.cap {
            // The new data alone exceeds capacity: keep only its tail.
            self.buf.clear();
            self.buf.extend(&data[data.len() - self.cap..]);
            return;
        }
        let overflow = (self.buf.len() + data.len()).saturating_sub(self.cap);
        for _ in 0..overflow {
            self.buf.pop_front();
        }
        self.buf.extend(data);
    }

    /// Current contents as a contiguous byte vector (for replay-on-attach).
    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
}
