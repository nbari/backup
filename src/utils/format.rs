use std::time::Duration;

#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();

    let days = secs / 86_400; // Number of seconds in a day
    let hours = (secs % 86_400) / 3_600; // Remaining seconds divided by 3600 for hours
    let minutes = (secs % 3_600) / 60; // Remaining seconds divided by 60 for minutes
    let seconds = secs % 60; // Remaining seconds

    if days > 0 {
        format!("{days}d {hours}h {minutes}m {seconds}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}
