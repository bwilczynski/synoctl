//! `docker updates`: hlctl's updates_list.tmpl and
//! updates_get_container.tmpl, without DEVICE, NAME and TYPE.

use jiff::tz::TimeZone;

use crate::model::docker::{UpdateDetail, UpdateList};
use crate::render::format::format_time;
use crate::render::{Table, fields, style};

pub fn list(l: &UpdateList, color: bool) -> String {
    let mut t = Table::new();
    t.row(&["ID", "STATUS", "CURRENT", "LATEST"]);
    for u in &l.items {
        t.row(&[
            &u.id,
            &style::update_status(u.status, color),
            &u.current_version,
            &u.latest_version,
        ]);
    }
    t.finish()
}

pub fn get(d: &UpdateDetail, tz: &TimeZone, color: bool) -> String {
    let mut t = Table::new();
    fields(
        &mut t,
        &[
            ("ID", d.id.clone()),
            ("IMAGE", d.image.clone()),
            ("STATUS", style::update_status(d.status, color)),
            ("CURRENT", d.current_version.clone()),
            ("LATEST", d.latest_version.clone()),
            ("SOURCE", d.source.clone()),
            ("RELEASE", d.release_url.clone()),
            ("PUBLISHED", format_time(d.published_at, tz)),
            ("CHECKED", format_time(d.checked_at, tz)),
        ],
    );
    t.finish()
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::tz::TimeZone;

    use super::*;
    use crate::releases::{FixtureReleases, Release, Repo};
    use crate::render::testutil::strip_ansi;
    use crate::service::Service;
    use crate::service::testutil::{fixtures, svc};

    fn now() -> Timestamp {
        "2026-04-25T12:00:00Z".parse().unwrap()
    }

    fn service() -> Service {
        let rel = Release {
            tag_name: "v2.7.0".into(),
            html_url: "https://github.com/immich-app/immich-machine-learning/releases/tag/v2.7.0"
                .into(),
            published_at: "2026-04-20T10:00:00Z".parse().unwrap(),
        };
        svc(fixtures()).with_updates(
            Box::new(FixtureReleases::of([(
                Repo::github("immich-app/immich-machine-learning"),
                rel,
            )])),
            vec![],
            String::new(),
        )
    }

    #[test]
    fn list_table() {
        let l = service().list_updates(None, false, now()).unwrap().data;
        assert_eq!(
            list(&l, false),
            concat!(
                "ID                       STATUS           CURRENT  LATEST\n",
                "immich_server            stale            v2.6.3   v2.6.3\n",
                "immich_machine_learning  updateAvailable  v2.6.3   v2.7.0\n",
                "prometheus               unknown          v3.2.1   v3.2.1\n",
            )
        );
        assert_eq!(strip_ansi(&list(&l, true)), list(&l, false));
    }

    #[test]
    fn get_table() {
        let d = service()
            .get_update("immich_machine_learning", now())
            .unwrap()
            .data;
        assert_eq!(
            get(&d, &TimeZone::UTC, false),
            concat!(
                "FIELD      VALUE\n",
                "ID         immich_machine_learning\n",
                "IMAGE      ghcr.io/immich-app/immich-machine-learning\n",
                "STATUS     updateAvailable\n",
                "CURRENT    v2.6.3\n",
                "LATEST     v2.7.0\n",
                "SOURCE     https://github.com/immich-app/immich-machine-learning\n",
                "RELEASE    https://github.com/immich-app/immich-machine-learning/releases/tag/v2.7.0\n",
                "PUBLISHED  2026-04-20 10:00\n",
                "CHECKED    2026-04-25 12:00\n",
            )
        );
    }
}
