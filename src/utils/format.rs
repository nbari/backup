use std::time::Duration;

pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();

    let days = secs / 86_400; // Number of seconds in a day
    let hours = (secs % 86_400) / 3_600; // Remaining seconds divided by 3600 for hours
    let minutes = (secs % 3_600) / 60; // Remaining seconds divided by 60 for minutes
    let seconds = secs % 60; // Remaining seconds

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
