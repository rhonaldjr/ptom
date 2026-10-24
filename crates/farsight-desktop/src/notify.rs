//! Desktop notifications and the connection approval prompt (U-2).

use std::time::Duration;

/// How long the person at the screen has to answer before access is denied.
pub const ACCESS_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Shows an informational notification. Failures are logged, not returned:
/// a missing notification daemon must never break a session.
pub async fn announce(summary: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        let result = notify_rust::Notification::new()
            .appname("Farsight")
            .summary(summary)
            .body(body)
            .icon("video-display")
            .show_async()
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "could not show a notification");
        }
    }
    #[cfg(not(target_os = "linux"))]
    tracing::info!(summary, body, "notification");
}

/// Asks the person at the screen whether `who` may connect. Returns `true`
/// only on an explicit "Allow"; a timeout, dismissal or missing notification
/// service means **deny** (council condition: prompts never default to accept).
pub async fn ask_access(who: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let shown = notify_rust::Notification::new()
            .appname("Farsight")
            .summary("Someone wants to see your screen")
            .body(&format!(
                "{who} is asking to view and control this desktop. Allow only if you expect this. \
                 It will be denied automatically in {} seconds.",
                ACCESS_PROMPT_TIMEOUT.as_secs()
            ))
            .icon("dialog-warning")
            .action("allow", "Allow")
            .action("deny", "Deny")
            .urgency(notify_rust::Urgency::Critical)
            .timeout(notify_rust::Timeout::Milliseconds(
                u32::try_from(ACCESS_PROMPT_TIMEOUT.as_millis()).unwrap_or(30_000),
            ))
            .show_async()
            .await;
        let handle = match shown {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%error, "cannot show the access prompt; denying");
                return false;
            }
        };
        let mut allowed = false;
        let wait = handle.wait_for_action_async(|response| {
            allowed = matches!(response, notify_rust::NotificationResponse::Action(action) if action == "allow");
        });
        if tokio::time::timeout(ACCESS_PROMPT_TIMEOUT, wait)
            .await
            .is_err()
        {
            return false;
        }
        allowed
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = who;
        false
    }
}
