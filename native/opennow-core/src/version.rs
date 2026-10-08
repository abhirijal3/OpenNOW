pub const APPLICATION_VERSION: &str = match option_env!("OPENNOW_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_version_is_semver() {
        semver::Version::parse(APPLICATION_VERSION).expect("application version must be semver");
    }
}
