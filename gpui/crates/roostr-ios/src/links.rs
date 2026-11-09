//! Deep links: `roostr://app/<web path>` opens that page.

/// A deep link's web path: `roostr://app/app/object/<id>` → `/app/object/<id>`
/// (the Swift app served the site at `roostr://app/`).
pub fn link_path(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("roostr://")?;
    let path = rest.strip_prefix("app").unwrap_or(rest);
    path.starts_with('/').then_some(path)
}

#[cfg(test)]
mod tests {
    use super::link_path;

    #[test]
    fn deep_links_name_web_paths() {
        assert_eq!(
            link_path("roostr://app/app/object/abc"),
            Some("/app/object/abc")
        );
        assert_eq!(link_path("roostr:///app/graph"), Some("/app/graph"));
        assert_eq!(link_path("https://roostr.space/app"), None);
    }
}
