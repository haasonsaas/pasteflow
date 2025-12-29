use similar::TextDiff;

pub fn unified_diff(before: &str, after: &str) -> String {
    let diff = TextDiff::from_lines(before, after);
    diff.unified_diff()
        .context_radius(3)
        .header("before", "after")
        .to_string()
}

