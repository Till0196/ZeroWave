// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ソースのスレッド。選局も信号品質の問い合わせも画面のスレッドでは行わない（映像も音声も止まるので）。
//! 要求と応答は同じ [`Ticket`] で対応させ、画面のスレッドは最新のもの以外の応答を捨てる。

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use crate::filters::Container;
use crate::guide::Key;
use crate::input::source::{Bytes, Channel, Error, Source};

const POLL_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

#[derive(Debug)]
enum Order {
    Open {
        ticket: Ticket,
        channel: Box<Channel>,
        key: Key,
    },
    /// 開いているストリームを切って、チューナーを返す（切断しない限り coaxd はチューナーを確保したまま）。
    Release,
    Quit,
}

/// `Bytes` を含むので `Debug` は手動で実装する。
pub enum Answer {
    Opened {
        ticket: Ticket,
        name: String,
        key: Key,
        /// 同じ搬送波の局へ切り替えるときの目印。
        how: String,
        bytes: Bytes,
        container: Container,
    },
    Refused {
        ticket: Ticket,
        name: String,
        why: Error,
    },
    /// 要求が無くても一秒に一度送る。
    State {
        tuner: Option<String>,
        cnr: Option<f32>,
    },
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opened { name, .. } => write!(out, "Opened({name})"),
            Self::Refused { name, why, .. } => write!(out, "Refused({name}: {why})"),
            Self::State { .. } => out.write_str("State"),
        }
    }
}

type Boot = (Box<dyn Source>, Receiver<Order>, Sender<Answer>);

/// 要求したがまだ応答の無い選局。「選局しています」の表示はこれだけで決める。
#[derive(Debug, Clone)]
pub struct Tuning {
    pub ticket: Ticket,
    pub name: String,
    pub key: crate::guide::Key,
    pub began: Instant,
}

pub struct Supply {
    to: Sender<Order>,
    from: Receiver<Answer>,
    boot: Option<Boot>,
    thread: Option<crate::thread::Thread>,
    next: u64,
    tuning: Option<Tuning>,
    opened: Option<String>,
    name: String,
}

impl std::fmt::Debug for Supply {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Supply")
            .field("name", &self.name)
            .field("tuning", &self.tuning)
            .finish_non_exhaustive()
    }
}

impl Supply {
    pub fn new(source: Box<dyn Source>) -> Self {
        let name = source.name().to_owned();
        let (to, orders) = channel();
        let (back, from) = channel();
        Self {
            to,
            from,
            boot: Some((source, orders, back)),
            thread: None,
            next: 0,
            tuning: None,
            opened: None,
            name,
        }
    }

    pub fn hand(&mut self) {
        let Some((mut source, orders, back)) = self.boot.take() else {
            return;
        };
        match crate::thread::Thread::spawn("source", "ソース", move |_told| {
            serving(source.as_mut(), &orders, &back);
        }) {
            Ok(thread) => self.thread = Some(thread),
            Err(error) => tracing::warn!("ソースのスレッドを起動できない: {error}"),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn carries(&self, channel: &Channel) -> bool {
        self.tuning.is_none() && self.opened.as_deref() == Some(channel.how.as_str())
    }

    pub fn opened(&mut self, how: String) {
        self.opened = Some(how);
    }

    pub fn tune(&mut self, channel: Channel, key: Key) -> Ticket {
        let ticket = Ticket(self.next);
        self.next += 1;
        self.opened = None;
        self.tuning = Some(Tuning {
            ticket,
            name: channel.name.clone(),
            key,
            began: Instant::now(),
        });
        let _ = self.to.send(Order::Open {
            ticket,
            channel: Box::new(channel),
            key,
        });
        ticket
    }

    pub fn tuning(&self) -> Option<&Tuning> {
        self.tuning.as_ref()
    }

    pub fn is_current(&self, ticket: Ticket) -> bool {
        self.tuning
            .as_ref()
            .is_some_and(|tuning| tuning.ticket == ticket)
    }

    pub fn settle(&mut self) {
        self.tuning = None;
    }

    pub fn release(&mut self) {
        self.tuning = None;
        self.opened = None;
        let _ = self.to.send(Order::Release);
    }

    pub fn taken(&self) -> Vec<Answer> {
        self.from.try_iter().collect()
    }

    pub fn close(&mut self) {
        let _ = self.to.send(Order::Quit);
        if let Some(mut thread) = self.thread.take() {
            thread.join_within(crate::thread::PATIENCE);
        }
    }
}

/// このスレッドだけが [`Source`] に触る。
fn serving(source: &mut dyn Source, orders: &Receiver<Order>, back: &Sender<Answer>) {
    let mut polled = Instant::now();
    loop {
        let until = POLL_EVERY.saturating_sub(polled.elapsed());
        match orders.recv_timeout(until) {
            Ok(Order::Quit) | Err(RecvTimeoutError::Disconnected) => {
                // 終わるときも切る（`read` は相手が応答しないと待ち続ける）。
                source.interrupt();
                return;
            }
            Ok(order) => match newest_open(order, orders) {
                Order::Quit => {
                    source.interrupt();
                    return;
                }
                Order::Release => {
                    source.interrupt();
                    polled = Instant::now() - POLL_EVERY;
                }
                open => {
                    if serve(source, open, back).is_err() {
                        return;
                    }
                    polled = Instant::now() - POLL_EVERY;
                }
            },
            Err(RecvTimeoutError::Timeout) => {}
        }
        if polled.elapsed() >= POLL_EVERY {
            polled = Instant::now();
            let state = Answer::State {
                tuner: source.holding().or_else(|| Some(source.name().to_owned())),
                cnr: source.cnr(),
            };
            if back.send(state).is_err() {
                return;
            }
        }
    }
}

fn serve(source: &mut dyn Source, order: Order, back: &Sender<Answer>) -> Result<(), ()> {
    let Order::Open {
        ticket,
        channel,
        key,
    } = order
    else {
        return Ok(());
    };
    // 先に前のストリームを止める（チューナーが一つのソースは、前のストリームを開いている間は次を開けない）。
    source.interrupt();
    let name = channel.name.clone();
    let answer = match source.open(&channel) {
        Ok((bytes, container)) => Answer::Opened {
            ticket,
            name,
            key,
            how: channel.how.clone(),
            bytes,
            container,
        },
        Err(why) => Answer::Refused { ticket, name, why },
    };
    back.send(answer).map_err(|_| ())
}

/// キューを空にして、いちばん新しい選局だけを返す。`Quit` があればそれが先。
fn newest_open(order: Order, orders: &Receiver<Order>) -> Order {
    if matches!(order, Order::Quit) {
        return order;
    }
    let mut newest = order;
    for next in orders.try_iter() {
        if matches!(next, Order::Quit) {
            return Order::Quit;
        }
        newest = next;
    }
    newest
}
