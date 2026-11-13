use std::{
    collections::VecDeque,
    future::poll_fn,
    io::{self, Write},
    task::{Context, Poll},
    time::Duration,
};

use bootvine_screen::{
    boot_view::{BootProjection, project_boot},
    screen_components::{
        console_claim::Refresh,
        screen_application::{
            Command, Message, Model, ScreenView, Subscription, subscriptions, update, view,
        },
    },
};
use bootvine_systemd_link::link_error::Error;
use crossterm::{
    cursor::{MoveTo, Show},
    execute,
    style::ResetColor,
    terminal::{Clear, ClearType, size},
};
use futures::{StreamExt, stream::LocalBoxStream};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
    widgets::Widget,
};

const INPUT_BATCH: usize = 64;
const PROJECTION_REFRESH: Duration = Duration::from_millis(100);
const ROWS_LEFT_FOR_SHELL: u16 = 8;

pub trait ScreenRuntime {
    fn perform(&mut self, command: Command) -> Vec<Message>;
    fn subscribe(&mut self, subscription: &Subscription) -> LocalBoxStream<'static, Message>;
    fn now(&self) -> Duration;
    fn drawn(&mut self) {}
}

enum SubscriptionSource {
    Running(LocalBoxStream<'static, Message>),
    Exhausted,
}

struct ActiveSubscription {
    subscription: Subscription,
    source: SubscriptionSource,
}

enum SourcePoll {
    Delivered,
    Waiting,
    Ended,
}

#[derive(Default)]
struct SubscriptionSet {
    active: Vec<ActiveSubscription>,
    next_start: usize,
}

impl SubscriptionSet {
    fn sync(&mut self, wanted: Vec<Subscription>, runtime: &mut impl ScreenRuntime) {
        self.active
            .retain(|active| wanted.contains(&active.subscription));
        wanted.into_iter().for_each(|subscription| {
            if !self
                .active
                .iter()
                .any(|active| active.subscription == subscription)
            {
                let source = SubscriptionSource::Running(runtime.subscribe(&subscription));
                self.active.push(ActiveSubscription {
                    subscription,
                    source,
                });
            }
        });
    }

    fn drain_source(
        source: &mut LocalBoxStream<'static, Message>,
        context: &mut Context<'_>,
        ready: &mut Vec<Message>,
    ) -> SourcePoll {
        while ready.len() < INPUT_BATCH {
            match source.poll_next_unpin(context) {
                Poll::Ready(Some(message)) => ready.push(message),
                Poll::Ready(None) => return SourcePoll::Ended,
                Poll::Pending => return SourcePoll::Waiting,
            }
        }
        SourcePoll::Delivered
    }

    fn poll_ready_messages(&mut self, context: &mut Context<'_>) -> Poll<Vec<Message>> {
        let count = self.active.len();
        let mut ready = Vec::new();
        for offset in 0..count {
            let active = &mut self.active[(self.next_start + offset) % count];
            let SubscriptionSource::Running(source) = &mut active.source else {
                continue;
            };
            match Self::drain_source(source, context, &mut ready) {
                SourcePoll::Ended => active.source = SubscriptionSource::Exhausted,
                SourcePoll::Delivered => break,
                SourcePoll::Waiting => {}
            }
        }
        self.next_start = (self.next_start + 1) % count.max(1);
        if ready.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(ready)
        }
    }
}

enum ProjectionCache {
    Built {
        projection: BootProjection,
        built_at: Duration,
    },
    Stale,
}

impl ProjectionCache {
    #[expect(
        clippy::disallowed_methods,
        reason = "boot timestamps mix systemd samples with this process's clock reads and can \
                  arrive out of order; treating such a projection as fresh keeps the screen up"
    )]
    fn current(&mut self, model: &Model, now: Duration) -> &BootProjection {
        if let Self::Built { built_at, .. } = self
            && now.saturating_sub(*built_at) >= PROJECTION_REFRESH
        {
            *self = Self::Stale;
        }
        if let Self::Stale = self {
            *self = Self::Built {
                projection: project_boot(&model.boot, &model.timing, now),
                built_at: now,
            };
        }
        match self {
            Self::Built { projection, .. } => projection,
            Self::Stale => unreachable!("a stale projection was rebuilt just above"),
        }
    }
}

pub async fn run_screen<Runtime: ScreenRuntime, Console: Backend>(
    model: &mut Model,
    runtime: &mut Runtime,
    terminal: &mut Terminal<Console>,
) -> Result<(), Error>
where
    Error: From<Console::Error>,
{
    let mut active_subscriptions = SubscriptionSet::default();
    let mut projection = ProjectionCache::Stale;
    loop {
        active_subscriptions.sync(subscriptions(model), runtime);
        let batch = poll_fn(|context| active_subscriptions.poll_ready_messages(context)).await;
        let mut queue = VecDeque::from(batch);
        while let Some(message) = queue.pop_front() {
            if matches!(message, Message::Boot(_)) {
                projection = ProjectionCache::Stale;
            }
            let commands = update(model, message, runtime.now());
            commands
                .into_iter()
                .for_each(|command| queue.extend(runtime.perform(command)));
            if model.handover.has_ended() {
                return Ok(());
            }
        }
        draw_screen(model, runtime, terminal, &mut projection)?;
    }
}

fn draw_screen<Runtime: ScreenRuntime, Console: Backend>(
    model: &mut Model,
    runtime: &mut Runtime,
    terminal: &mut Terminal<Console>,
    projection: &mut ProjectionCache,
) -> Result<(), Error>
where
    Error: From<Console::Error>,
{
    let now = runtime.now();
    let refresh = model.console.refresh;
    let palette = model.console.look.palette;
    let ScreenView::Boot(boot_view) = view(model, projection.current(model, now), now) else {
        return Ok(());
    };
    if refresh == Refresh::Clear {
        terminal.clear()?;
    }
    let completed = terminal.draw(|canvas| {
        canvas.render_widget(boot_view, canvas.area());
        palette.fit_frame(canvas.buffer_mut());
    })?;
    if refresh == Refresh::Repaint {
        let painted = completed.buffer.clone();
        overwrite_every_cell(terminal.backend_mut(), &painted)?;
    }
    runtime.drawn();
    let _ = update(model, Message::Drawn, now);
    Ok(())
}

pub fn print_shell_summary(model: &Model, now: Duration) -> io::Result<()> {
    let unused_projection = BootProjection::default();
    let ScreenView::ShellSummary(summary) = view(model, &unused_projection, now) else {
        return Ok(());
    };
    let (columns, rows) = size()?;
    #[expect(
        clippy::disallowed_methods,
        reason = "a terminal shorter than the rows kept for the shell gets a summary of zero rows"
    )]
    let most_rows = rows.saturating_sub(ROWS_LEFT_FOR_SHELL);
    let area = Rect::new(0, 0, columns, summary.height().min(most_rows));
    let mut printed = Buffer::empty(area);
    summary.render(area, &mut printed);
    model.console.look.palette.fit_frame(&mut printed);
    let mut console = io::stdout();
    execute!(console, Clear(ClearType::All), MoveTo(0, 0))?;
    overwrite_every_cell(&mut CrosstermBackend::new(io::stdout()), &printed)?;
    execute!(console, ResetColor, MoveTo(0, area.height + 1), Show)?;
    console.flush()
}

pub(crate) fn overwrite_every_cell<Console: Backend>(
    backend: &mut Console,
    frame: &Buffer,
) -> Result<(), Console::Error> {
    backend.draw(
        frame
            .content
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.diff_option != CellDiffOption::Skip)
            .map(|(index, cell)| {
                let (column, row) = frame.pos_of(index);
                (column, row, cell)
            }),
    )?;
    backend.flush()
}
