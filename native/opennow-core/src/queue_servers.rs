pub(super) fn zone_url(zone: &str) -> Option<String> {
    if !(9..=14).contains(&zone.len()) {
        return None;
    }
    let parts = zone.split('-').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0] != "NP"
        || !(3..=8).contains(&parts[1].len())
        || !parts[1]
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        || !parts[1].bytes().next()?.is_ascii_uppercase()
        || parts[2].len() != 2
        || !parts[2].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(format!(
        "https://{}.cloudmatchbeta.nvidiagrid.net/",
        zone.to_ascii_lowercase()
    ))
}
