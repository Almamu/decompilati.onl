/// A supported platform. `color` is used for the platform chip and image placeholder.
pub struct Platform {
    pub slug: &'static str,
    pub name: &'static str,
    pub short: &'static str,
    pub maker: &'static str,
    pub color: &'static str,
}

macro_rules! platforms {
    ($( $slug:literal, $name:literal, $short:literal, $maker:literal, $color:literal; )*) => {
        pub static PLATFORMS: &[Platform] = &[
            $( Platform { slug: $slug, name: $name, short: $short, maker: $maker, color: $color }, )*
        ];
    };
}

platforms! {
    // Nintendo home consoles
    "nes", "Nintendo Entertainment System", "NES", "Nintendo", "#b3262d";
    "snes", "Super Nintendo", "SNES", "Nintendo", "#6c5ba7";
    "n64", "Nintendo 64", "N64", "Nintendo", "#1f8a3c";
    "gc", "GameCube", "GC", "Nintendo", "#5a4fcf";
    "wii", "Wii", "Wii", "Nintendo", "#3aa6dd";
    "wiiu", "Wii U", "Wii U", "Nintendo", "#0098c8";
    "switch", "Nintendo Switch", "Switch", "Nintendo", "#e60012";
    // Nintendo handhelds
    "gb", "Game Boy", "GB", "Nintendo", "#7a8a2e";
    "gbc", "Game Boy Color", "GBC", "Nintendo", "#8e44ad";
    "gba", "Game Boy Advance", "GBA", "Nintendo", "#4b3ea8";
    "nds", "Nintendo DS", "DS", "Nintendo", "#6b7280";
    "3ds", "Nintendo 3DS", "3DS", "Nintendo", "#c8102e";
    // Sony
    "ps1", "PlayStation", "PS1", "Sony", "#7d8597";
    "ps2", "PlayStation 2", "PS2", "Sony", "#1d3c8f";
    "ps3", "PlayStation 3", "PS3", "Sony", "#2b2b2b";
    "ps4", "PlayStation 4", "PS4", "Sony", "#003791";
    "ps5", "PlayStation 5", "PS5", "Sony", "#2a5bd7";
    "psp", "PlayStation Portable", "PSP", "Sony", "#3d3d3d";
    "psvita", "PlayStation Vita", "Vita", "Sony", "#1b4fa0";
    // Sega
    "sms", "Master System", "SMS", "Sega", "#d1202f";
    "gg", "Game Gear", "GG", "Sega", "#333f48";
    "md", "Mega Drive / Genesis", "MD", "Sega", "#17171a";
    "segacd", "Sega CD / Mega-CD", "SCD", "Sega", "#3b3b44";
    "32x", "Sega 32X", "32X", "Sega", "#a0141c";
    "saturn", "Saturn", "SAT", "Sega", "#4f5d75";
    "dc", "Dreamcast", "DC", "Sega", "#f26722";
    // Microsoft
    "xbox", "Xbox", "XBOX", "Microsoft", "#107c10";
    "x360", "Xbox 360", "X360", "Microsoft", "#5dc21e";
    "xone", "Xbox One", "XONE", "Microsoft", "#0e7a0d";
    // Others
    "pce", "PC Engine / TurboGrafx-16", "PCE", "NEC", "#e05a1c";
    "neogeo", "Neo Geo", "NEO", "SNK", "#c9a227";
    "arcade", "Arcade", "ARC", "Other", "#8a2be2";
    "pc", "PC", "PC", "Other", "#4a5568";
    "other", "Other", "?", "Other", "#718096";
}

pub fn get(slug: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|p| p.slug == slug)
}

pub fn get_or_other(slug: &str) -> &'static Platform {
    get(slug).unwrap_or_else(|| get("other").unwrap())
}
