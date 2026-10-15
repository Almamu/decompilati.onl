//! `decompsite seed-demo`: fills an empty database with fake projects for local development.

use rand::Rng;

use crate::{
    db::{self, Db, ProjectMeta},
    report::{AreaInput, CodeStats, DataStats, FunctionStats, ReportInput},
    util::{new_api_key, now},
};

const DEMO: &[(&str, &str, &str, bool)] = &[
    ("Ocarina of Time", "n64", "Overworld", false),
    ("Super Mario Sunshine", "gc", "Isle Delfino", false),
    ("Pokémon Emerald", "gba", "Hoenn", false),
    ("Twilight Princess", "wii", "Hyrule", false),
    ("Crash Bandicoot", "ps1", "N. Sanity Beach", false),
    ("Sonic Adventure 2", "dc", "City Escape", true),
    ("Kingdom Hearts", "ps2", "Traverse Town", false),
    ("Halo: Combat Evolved", "xbox", "Pillar of Autumn", true),
    ("Link's Awakening", "gb", "Koholint", false),
    ("Metroid Prime Hunters", "nds", "Alinos", false),
];

pub fn run(db: &Db, uploads: &std::path::Path) -> anyhow::Result<()> {
    let mut c = db.conn()?;
    let user = db::upsert_user(&mut c, 1, "demo-user", Some("Demo User"), None)?;
    let mut rng = rand::thread_rng();
    for (name, platform, zone, ai) in DEMO {
        let slug = crate::util::slugify(name);
        if db::project_by_slug(&mut c, &slug)?.is_some() {
            continue;
        }
        let (key, hash, prefix) = new_api_key();
        let meta = ProjectMeta {
            slug: slug.clone(),
            name: name.to_string(),
            platform: platform.to_string(),
            repo: format!("demo/{slug}"),
            github_repo_id: None,
            repo_host: "github".into(),
            repo_url: None,
            ai_contributions: String::new(),
            discord: None,
            description: format!("A demo decompilation of {name}."),
            ai_generated: *ai,
        };
        let image = demo_cover(platform, uploads)?;
        let id = db::insert_project(&mut c, user.id, &meta, &image, &hash, &prefix)?;
        println!("{slug}: {key}");

        // Several reports spread over the last 60 days, with progress going up.
        let mut progress: f64 = rng.gen_range(0.05..0.5);
        let reports = rng.gen_range(3..12);
        for i in 0..reports {
            progress = (progress + rng.gen_range(0.0..0.06)).min(1.0);
            let areas = (0..rng.gen_range(4..10))
                .map(|a| {
                    let total = rng.gen_range(20_000..400_000u64);
                    let p = (progress + rng.gen_range(-0.2..0.2)).clamp(0.0, 1.0);
                    let matching = (total as f64 * p * 0.85) as u64;
                    let non_matching = (total as f64 * p * 0.15) as u64;
                    let children = if a == 0 {
                        (0..4)
                            .map(|k| AreaInput {
                                name: format!("{zone} part {}", k + 1),
                                description: None,
                                code: Some(CodeStats { total: total / 4, matching: matching / 4, non_matching: non_matching / 4, linked: matching / 8 }),
                                data: None,
                                functions: Some(FunctionStats { total: total / 1600, matching: matching / 1600 }),
                                weight: None,
                                children: vec![],
                            })
                            .collect()
                    } else {
                        vec![]
                    };
                    AreaInput {
                        name: if a == 0 { zone.to_string() } else { format!("Overlay {a}") },
                        description: Some("Demo area".into()),
                        code: if children.is_empty() { Some(CodeStats { total, matching, non_matching, linked: matching / 2 }) } else { None },
                        data: Some(DataStats { total: total / 4, done: (total as f64 / 4.0 * p) as u64 }),
                        functions: children.is_empty().then(|| FunctionStats { total: total / 400, matching: matching / 400 }),
                        weight: None,
                        children,
                    }
                })
                .collect();
            let report = ReportInput {
                version: None,
                functions: None,
                unit: Some("bytes".into()),
                code: None,
                data: None,
                areas,
                layout: None,
                commit: Some(format!("{:010x}", rng.r#gen::<u64>() & 0xff_ffff_ffff)),
                message: Some(format!("Demo report #{}", i + 1)),
            }
            .normalize()
            .map_err(anyhow::Error::msg)?;
            let project = db::project_by_id(&mut c, id)?.unwrap();
            // Spread the reports over the past weeks so the history chart has a time axis.
            let ts = now() - (reports - i) as i64 * 5 * 86400 - rng.gen_range(0..86400);
            db::apply_report(&mut c, &project, &report, Some(ts))?;
        }
    }
    Ok(())
}


/// A plain gradient in the platform's colour, stored like a real upload.
fn demo_cover(platform: &str, uploads: &std::path::Path) -> anyhow::Result<String> {
    let hex = crate::platforms::get_or_other(platform).color.trim_start_matches('#');
    let c = u32::from_str_radix(hex, 16)?;
    let (r, g, b) = ((c >> 16) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32);
    let img = image::RgbImage::from_fn(640, 360, |x, y| {
        let t = 0.55 + 0.45 * (1.0 - (x + y) as f32 / 1000.0);
        image::Rgb([(r * t) as u8, (g * t) as u8, (b * t) as u8])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    crate::images::store_cover(&png, uploads).map_err(anyhow::Error::msg)
}
