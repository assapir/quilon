// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! The reactor: a thin wrapper over a `mio` poll loop. It drives three kinds of
//! readiness for the scheduler:
//!
//!  * timed waits — the nearest sleep deadline becomes the poll timeout, so the
//!    scheduler wakes in time to resume due `sleep`ers;
//!  * source readiness — non-blocking sources (a TCP socket in [`crate::net`] today,
//!    files/pipes later) are registered with a unique [`Token`]; when `poll` reports
//!    a token ready the scheduler resumes the one fiber parked on it; and
//!  * a helper OS thread's completion — a thread with no `mio::event::Source` of its own (a
//!    background computation, e.g. [`crate::net`]'s hostname resolver) signals one via
//!    [`ReactorWaker`].
//!
//! A single `Poll::poll` services all three: the timeout bounds how long it blocks, and
//! any source (or waker) that becomes ready before then returns it early. `EINTR` and
//! spurious wakeups are harmless — the scheduler re-checks timers and only wakes fibers
//! whose token actually fired.

use mio::event::Source;
use mio::{Events, Interest, Poll, Token, Waker};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The token mio's `Waker` itself is registered under — reserved (never handed out by
/// [`Reactor::alloc_token`], which starts at 0 and only counts up) so it can never collide with
/// a real per-source token. Nothing ever parks on it directly: seeing it among the ready tokens
/// means only "check [`ReactorWaker`]'s completions", handled entirely inside
/// [`Reactor::ready_tokens`].
const WAKER_TOKEN: Token = Token(usize::MAX);

/// A handle for a helper OS thread — one with no `mio::event::Source` of its own to register —
/// to tell the reactor that `token` (allocated for it via [`Reactor::alloc_token`]) is ready.
/// Cheap to clone (both fields are `Arc`s), so every such helper gets its own clone.
///
/// mio allows only one live [`Waker`] per `Poll` ("what happens if multiple `Waker`s are
/// registered with the same `Poll` is unspecified" — its own docs), so every helper shares the
/// reactor's single `Waker` and instead records which of possibly several pending tokens
/// actually finished in `completed`, a list [`Reactor::ready_tokens`] drains on the next wake.
#[derive(Clone)]
pub struct ReactorWaker {
    completed: Arc<Mutex<Vec<Token>>>,
    waker: Arc<Waker>,
}

impl ReactorWaker {
    /// Record `token` as ready and wake the reactor so the scheduler notices it on its next
    /// turn — the last thing a helper thread does before finishing.
    pub fn complete(&self, token: Token) {
        self.completed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(token);
        let _ = self.waker.wake();
    }
}

pub struct Reactor {
    poll: Poll,
    events: Events,
    /// Monotonic token allocator. Tokens are never reused; a `usize` counter is
    /// ample and keeps the token/fiber map unambiguous even after sources close.
    next_token: usize,
    completed: Arc<Mutex<Vec<Token>>>,
    waker: Arc<Waker>,
}

impl Reactor {
    pub fn new() -> io::Result<Self> {
        let poll = Poll::new()?;
        let waker = Arc::new(Waker::new(poll.registry(), WAKER_TOKEN)?);
        Ok(Self {
            poll,
            events: Events::with_capacity(64),
            next_token: 0,
            completed: Arc::new(Mutex::new(Vec::new())),
            waker,
        })
    }

    /// Hand out a fresh, never-reused token for a new source.
    pub fn alloc_token(&mut self) -> Token {
        let token = Token(self.next_token);
        self.next_token += 1;
        token
    }

    pub fn register(
        &self,
        source: &mut impl Source,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        self.poll.registry().register(source, token, interest)
    }

    pub fn reregister(
        &self,
        source: &mut impl Source,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        self.poll.registry().reregister(source, token, interest)
    }

    pub fn deregister(&self, source: &mut impl Source) -> io::Result<()> {
        self.poll.registry().deregister(source)
    }

    /// A fresh [`ReactorWaker`] sharing this reactor's one `Waker`, for a helper OS thread with
    /// no `Source` of its own to signal a token's completion.
    pub fn resolver_waker(&self) -> ReactorWaker {
        ReactorWaker {
            completed: Arc::clone(&self.completed),
            waker: Arc::clone(&self.waker),
        }
    }

    /// Block until `timeout` elapses or a registered source (or the [`ReactorWaker`]) is ready.
    /// `None` blocks indefinitely; the scheduler passes `None` only when fibers are parked on
    /// source readiness with no pending timer.
    pub fn wait(&mut self, timeout: Option<Duration>) {
        self.events.clear();
        let _ = self.poll.poll(&mut self.events, timeout);
    }

    /// Tokens whose sources became ready in the last [`wait`](Self::wait), plus any token a
    /// helper thread recorded via [`ReactorWaker::complete`] since the last call — draining
    /// that list here is what turns "the shared waker fired" into "these specific tokens are
    /// ready", so the scheduler maps each back to the fiber parked on it exactly as it would a
    /// directly-polled source's.
    pub fn ready_tokens(&self) -> impl Iterator<Item = Token> + '_ {
        let completed: Vec<Token> = std::mem::take(
            &mut *self
                .completed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        self.events
            .iter()
            .map(|event| event.token())
            .chain(completed)
    }
}
