//! Natural-place name pool used to generate readable agent ids.
//!
//! Random short hashes (`b5a471ff`) were unique but anonymous —
//! impossible to refer to in conversation, hard to spot in a directory
//! listing of `workspaces/`. Names of mountains, deserts, parks, and
//! seas give every agent a distinct identity that's easy to read,
//! type, and remember. (We avoid city names because Conductor already
//! uses them.) On the rare collision we suffix `-2`, `-3`, …

use std::collections::HashSet;

pub const PLACES: &[&str] = &[
    // Mountains & ranges
    "everest",
    "kilimanjaro",
    "denali",
    "fuji",
    "etna",
    "vesuvius",
    "olympus",
    "rainier",
    "matterhorn",
    "jungfrau",
    "dolomites",
    "atlas",
    "andes",
    "pyrenees",
    "carpathians",
    "caucasus",
    "himalaya",
    "karakoram",
    "rockies",
    "appalachians",
    "alps",
    "fitzroy",
    "blanc",
    "aconcagua",
    "cotopaxi",
    "shasta",
    "annapurna",
    "blue-ridge",
    "blue-mountains",
    "langtang",
    "nilgiri",
    "ruwenzori",
    "seorak",
    // Volcanoes
    "krakatoa",
    "tambora",
    "mauna-loa",
    "mauna-kea",
    "sakurajima",
    "popocatepetl",
    "kilauea",
    "arenal",
    "chimborazo",
    "hallasan",
    // National parks & wildernesses
    "yosemite",
    "yellowstone",
    "zion",
    "banff",
    "jasper",
    "sequoia",
    "redwood",
    "bryce",
    "kruger",
    "serengeti",
    "plitvice",
    "fiordland",
    "daintree",
    "etosha",
    "ngorongoro",
    "acadia",
    "everglades",
    "kakadu",
    "komodo",
    "tongariro",
    "jiuzhaigou",
    "bromo",
    "bwindi",
    "glacier-bay",
    "manuel-antonio",
    "masaimara",
    "monteverde",
    "samburu",
    "snowdonia",
    "virunga",
    // Deserts
    "sahara",
    "gobi",
    "kalahari",
    "atacama",
    "mojave",
    "sonoran",
    "namib",
    "thar",
    "negev",
    "danakil",
    "sossusvlei",
    "wadi-rum",
    "death-valley",
    // Lakes & inland seas
    "titicaca",
    "tanganyika",
    "victoria",
    "malawi",
    "garda",
    "dead-sea",
    "galilee",
    "inle",
    "tekapo",
    "wanaka",
    // Islands & archipelagos
    "borneo",
    "sumatra",
    "bali",
    "madagascar",
    "sardinia",
    "sicily",
    "corsica",
    "crete",
    "mallorca",
    "santorini",
    "capri",
    "zanzibar",
    "mauritius",
    "seychelles",
    "maldives",
    "galapagos",
    "hawaii",
    "tahiti",
    "fiji",
    "faroe",
    "lofoten",
    "hebrides",
    "aleutians",
    "palawan",
    "socotra",
    "jeju",
    "skye",
    "madeira",
    "azores",
    "canaries",
    "okinawa",
    "yakushima",
    "tasmania",
    "whitsunday",
    "baffin",
    "bohol",
    "boracay",
    "easter",
    "gomera",
    "ibiza",
    "kangaroo",
    "lombok",
    "raja-ampat",
    "phi-phi",
    "phuket",
    "svalbard",
    // Rivers & valleys
    "napa",
    "sonoma",
    "douro",
    "loire",
    "rhone",
    "yangtze",
    "mekong",
    "nile",
    "amazon",
    "mississippi",
    "colorado",
    "columbia",
    "danube",
    "hudson",
    "mosel",
    "iguazu",
    "okavango",
    "guilin",
    "hunza",
    "uyuni",
    "orinoco",
    "rubicon",
    // Regions, plateaus & coasts
    "patagonia",
    "tuscany",
    "andalusia",
    "provence",
    "cornwall",
    "brittany",
    "normandy",
    "galicia",
    "basque",
    "tibet",
    "transylvania",
    "cappadocia",
    "altiplano",
    "deccan",
    "anatolia",
    "ozarks",
    "yucatan",
    "cascadia",
    "badlands",
    "adirondacks",
    "chesapeake",
    "pantanal",
    "pampas",
    "cerrado",
    "maghreb",
    "sahel",
    "levant",
    "sinai",
    "arabia",
    "pamir",
    "ladakh",
    "kerala",
    "goa",
    "konkan",
    "malabar",
    "sundarbans",
    "hokkaido",
    "kansai",
    "annamite",
    "cardamom",
    "kimberley",
    "pilbara",
    "outback",
    "algarve",
    "alentejo",
    "alsace",
    "burgundy",
    "champagne",
    "bavaria",
    "blackforest",
    "bohemia",
    "moravia",
    "umbria",
    "liguria",
    "piedmont",
    "apulia",
    "asturias",
    "cantabria",
    "occitania",
    "lombardy",
    "savoy",
    "assam",
    "camargue",
    "cancun",
    "chiapas",
    "cotswolds",
    "dordogne",
    "flanders",
    "great-ocean",
    "hakone",
    "highlands",
    "jericho",
    "kashmir",
    "kamakura",
    "krabi",
    "kyoto",
    "lake-district",
    "lijiang",
    "lorraine",
    "luang-prabang",
    "monterey",
    "mustang",
    "nara",
    "nunavut",
    "oaxaca",
    "rajasthan",
    "sichuan",
    "ventura",
    "wallonia",
    "xinjiang",
    "yunnan",
    "yukon",
    // Iconic landforms
    "gibraltar",
    "uluru",
    "halong",
    "amalfi",
    "petra",
    "angkor",
    "machu-picchu",
    "grand-canyon",
    "grand-teton",
    "big-sur",
    "cinque-terre",
    "el-capitan",
    "halfdome",
    "monument",
    "antelope",
    "canyonlands",
    "huangshan",
    "zhangjiajie",
    "nikko",
    "belize",
    "palau",
    "ningaloo",
    "great-barrier",
    "milford",
    "geiranger",
    "naeroy",
    "bagan",
    "copper-canyon",
    "ellora",
    "fox",
    "franz-josef",
    "goreme",
    "meteora",
    "pamukkale",
    "sigiriya",
    "tepui",
    "tikal",
    "torres",
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
    // Every random pick collided. With a ~300-name pool this is vanishingly
    // rare unless `used` is large — and `used` folds in every on-disk checkout
    // across all builds sharing the root, so hitting this points at namespace
    // saturation or a pile-up of orphaned dirs. Log loudly with the set size so
    // the cause is diagnosable rather than a silent `-2`.
    let base = pick_random();
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !used.contains(&candidate) {
            tracing::warn!(
                used = used.len(),
                pool = PLACES.len(),
                name = %candidate,
                "name allocator exhausted 10 random picks; falling back to a numbered suffix"
            );
            return candidate;
        }
        n += 1;
    }
}

/// Cheap random pick — folds the first 8 bytes of a fresh UUID into a
/// `u64` index. Avoids pulling in the `rand` crate for a single use.
///
/// (A single byte only spans 0–255, so it couldn't reach a pool larger
/// than 256 entries; a `u64` keeps every place reachable with negligible
/// modulo bias as the pool grows.)
fn pick_random() -> &'static str {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    let idx = (n % PLACES.len() as u64) as usize;
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
    fn pick_random_reaches_entire_pool() {
        // Regression guard: a single UUID byte only spans 0–255, so it
        // could never index a pool larger than 256 entries (the tail was
        // silently unreachable). Confirm every place can be drawn.
        let mut seen: HashSet<&str> = HashSet::new();
        for _ in 0..100_000 {
            seen.insert(pick_random());
        }
        assert_eq!(seen.len(), PLACES.len(), "some places are unreachable");
    }

    #[test]
    fn pool_has_no_duplicates() {
        let unique: HashSet<&&str> = PLACES.iter().collect();
        assert_eq!(unique.len(), PLACES.len(), "PLACES contains duplicates");
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
