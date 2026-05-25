//! Natural-place name pool used to generate readable agent ids.
//!
//! Random short hashes (`b5a471ff`) were unique but anonymous —
//! impossible to refer to in conversation, hard to spot in a directory
//! listing of `.worktrees/`. Names of mountains, deserts, parks, and
//! seas give every agent a distinct identity that's easy to read,
//! type, and remember. (We avoid city names because Conductor already
//! uses them.) On the rare collision we suffix `-2`, `-3`, …

use std::collections::HashSet;

pub const PLACES: &[&str] = &[
    // Mountains & ranges
    "everest", "kilimanjaro", "denali", "fuji", "etna", "vesuvius", "olympus",
    "rainier", "matterhorn", "jungfrau", "dolomites", "atlas", "andes",
    "pyrenees", "urals", "carpathians", "caucasus", "himalaya", "karakoram",
    "sierras", "rockies", "appalachians", "balkans", "taurus", "zagros",
    "alps",
    // Volcanoes
    "krakatoa", "tambora", "mauna-loa", "mauna-kea", "sakurajima",
    "popocatepetl", "cotopaxi", "aconcagua", "kilauea",
    // National parks & wildernesses
    "yosemite", "yellowstone", "zion", "banff", "jasper", "glacier",
    "sequoia", "arches", "bryce", "redwood", "olympic", "kruger", "serengeti",
    "plitvice", "fiordland", "daintree", "etosha", "ngorongoro",
    // Deserts
    "sahara", "gobi", "kalahari", "atacama", "mojave", "sonoran", "namib",
    "taklamakan", "thar", "karakum",
    // Lakes & inland seas
    "baikal", "caspian", "titicaca", "tanganyika", "balkhash", "victoria",
    "malawi", "eyre", "ladoga", "garda",
    // Islands & archipelagos
    "borneo", "sumatra", "java", "madagascar", "iceland", "greenland",
    "sardinia", "sicily", "corsica", "crete", "cyprus", "malta", "mallorca",
    "santorini", "capri", "zanzibar", "mauritius", "seychelles", "maldives",
    "galapagos", "hawaii", "tahiti", "fiji", "faroe", "lofoten", "orkney",
    "hebrides", "aleutians",
    // Rivers & valleys
    "napa", "sonoma", "douro", "loire", "rhone", "yangtze", "mekong", "nile",
    "amazon", "mississippi", "colorado", "columbia", "danube", "hudson",
    "mosel",
    // Plateaus & highlands
    "altiplano", "deccan", "anatolia", "cappadocia", "ozarks",
    // Regions & coasts
    "patagonia", "kamchatka", "siberia", "lapland", "tuscany", "andalusia",
    "provence", "cornwall", "brittany", "normandy", "galicia", "basque",
    "tibet", "transylvania",
    // Iconic landforms
    "gibraltar", "uluru", "halong",
];

/// Allocate a unique id from the city pool.
///
/// Strategy: try up to 10 random picks first — if any of them aren't
/// taken, use it. Most users have a handful of agents so this almost
/// always lands on a fresh name and gives the sidebar lots of visual
/// variety. If every random pick collides (small pool, busy user),
/// fall back to numbered suffixes (`tokyo-2`, `tokyo-3`, …) on a
/// final random pick.
pub fn allocate(used: &HashSet<String>) -> String {
    for _ in 0..10 {
        let candidate = pick_random();
        if !used.contains(candidate) {
            return candidate.to_string();
        }
    }
    let base = pick_random();
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !used.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Cheap random pick — uses the first byte of a fresh UUID as the
/// index. Avoids pulling in the `rand` crate for a single use.
fn pick_random() -> &'static str {
    let idx = uuid::Uuid::new_v4().as_bytes()[0] as usize % PLACES.len();
    PLACES[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_unused_name_on_empty_set() {
        let used = HashSet::new();
        let id = allocate(&used);
        assert!(PLACES.contains(&id.as_str()), "got {id}");
    }

    #[test]
    fn falls_back_to_suffix_when_pool_exhausted() {
        // Mark every place as used; allocator must produce a "<place>-N"
        // form rather than spinning forever.
        let used: HashSet<String> = PLACES.iter().map(|p| p.to_string()).collect();
        let id = allocate(&used);
        assert!(id.contains('-'), "expected suffixed id, got {id}");
        let (base, num) = id.rsplit_once('-').unwrap();
        assert!(PLACES.contains(&base), "base {base} not in pool");
        assert!(num.parse::<u32>().is_ok(), "suffix {num} not numeric");
    }

    #[test]
    fn suffix_skips_already_used_numbers() {
        // Exhaust the pool plus -2 and -3 of every base so the fallback
        // must scan past them.
        let mut used: HashSet<String> = PLACES.iter().map(|p| p.to_string()).collect();
        for n in 2..=3 {
            for &p in PLACES {
                used.insert(format!("{p}-{n}"));
            }
        }
        let id = allocate(&used);
        let num: u32 = id.rsplit('-').next().unwrap().parse().unwrap();
        assert!(num >= 4, "expected >= 4, got {id}");
    }
}
