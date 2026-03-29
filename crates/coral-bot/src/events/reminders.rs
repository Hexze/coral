use std::time::Duration;

use chrono::Utc;
use serenity::all::Context;
use tokio::sync::Notify;

use database::ReminderRepository;

use crate::commands::user::reminder::send_reminder_dm;
use crate::framework::Data;

const MAX_SLEEP_SECS: u64 = 300;

static REMINDER_NOTIFY: Notify = Notify::const_new();

pub fn wake_poller() {
    REMINDER_NOTIFY.notify_one();
}

pub fn spawn_reminder_poller(ctx: Context, data: Data) {
    tokio::spawn(async move {
        loop {
            let sleep_dur = match next_sleep_duration(&data).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("Reminder poll error: {e}");
                    Duration::from_secs(10)
                }
            };

            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => {}
                _ = REMINDER_NOTIFY.notified() => {}
            }

            if let Err(e) = fire_due_reminders(&ctx, &data).await {
                tracing::error!("Reminder fire error: {e}");
            }
        }
    });
}

async fn next_sleep_duration(data: &Data) -> anyhow::Result<Duration> {
    let repo = ReminderRepository::new(data.db.pool());
    let next = repo.next_due_at().await?;

    match next {
        Some(at) => {
            let delta = at - Utc::now();
            let secs = delta.num_milliseconds().max(0) as u64;
            Ok(Duration::from_millis(secs))
        }
        None => Ok(Duration::from_secs(MAX_SLEEP_SECS)),
    }
}

async fn fire_due_reminders(ctx: &Context, data: &Data) -> anyhow::Result<()> {
    let repo = ReminderRepository::new(data.db.pool());

    let due = repo.get_due().await?;
    let due_repeating = repo.get_due_repeating().await?;

    for reminder in due.iter().chain(due_repeating.iter()) {
        if let Err(e) = send_reminder_dm(ctx, reminder).await {
            tracing::warn!(
                "Failed to DM reminder #{} to {}: {e}",
                reminder.id,
                reminder.discord_id
            );
        }
    }

    Ok(())
}
