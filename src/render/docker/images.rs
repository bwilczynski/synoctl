//! `docker images`: hlctl's images_list.tmpl and images_get.tmpl, without
//! DEVICE.

use jiff::tz::TimeZone;

use crate::model::docker::{DockerImageDetail, DockerImageList};
use crate::render::format::{format_bytes, format_time};
use crate::render::{Table, fields};

pub fn list(l: &DockerImageList) -> String {
    let mut t = Table::new();
    t.row(&["ID", "REPOSITORY", "TAGS", "SIZE"]);
    for i in &l.items {
        t.row(&[
            &i.id,
            &i.repository,
            &i.tags.join(", "),
            &format_bytes(i.size),
        ]);
    }
    t.finish()
}

pub fn get(d: &DockerImageDetail, tz: &TimeZone) -> String {
    let mut rows = vec![
        ("ID", d.id.clone()),
        ("REPOSITORY", d.repository.clone()),
        ("TAGS", d.tags.join(", ")),
        ("SIZE", format_bytes(d.size)),
        ("VIRTUAL SIZE", format_bytes(d.virtual_size)),
        ("CREATED", format_time(d.created, tz)),
    ];
    if let Some(desc) = &d.description {
        rows.push(("DESCRIPTION", desc.clone()));
    }
    let mut t = Table::new();
    fields(&mut t, &rows);
    t.finish()
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::*;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_table() {
        let l = svc(fixtures()).list_images().unwrap();
        assert_eq!(
            list(&l),
            concat!(
                "ID            REPOSITORY                TAGS     SIZE\n",
                "aabbccddeeff  busybox                   latest   4.2 MB\n",
                "bbccddeeff00  gcr.io/cadvisor/cadvisor  v0.52.1  77.0 MB\n",
                "ccddeeff0011  eclipse-mosquitto         latest   6.4 MB\n",
                "ddeeff001122  certbot/dns-route53       latest   190.0 MB\n",
            )
        );
    }

    #[test]
    fn get_table() {
        let d = svc(fixtures()).get_image("bbccddeeff00").unwrap();
        assert_eq!(
            get(&d, &TimeZone::UTC),
            concat!(
                "FIELD         VALUE\n",
                "ID            bbccddeeff00\n",
                "REPOSITORY    gcr.io/cadvisor/cadvisor\n",
                "TAGS          v0.52.1\n",
                "SIZE          77.0 MB\n",
                "VIRTUAL SIZE  77.0 MB\n",
                "CREATED       2025-03-20 03:40\n",
            )
        );
    }
}
