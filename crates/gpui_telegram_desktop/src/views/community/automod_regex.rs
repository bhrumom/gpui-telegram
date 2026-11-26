//! Synchronous Rust `regex` syntax & bounded NFA expansion validator for AutoMod rules.
//!
//! Aligned with `gpui_telegram_community::AutoModEngine`'s `regex::RegexBuilder` configuration
//! (`case_insensitive(true)`, `utf8(true)`, `size_limit(64 * 1024)`, `dfa_size_limit(64 * 1024)`).

/// Exact UAX44-LM3 symbolic property/value normalization matching `regex-syntax`.
fn symbolic_name_normalize(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let starts_with_is = bytes.len() >= 2
        && bytes[0].eq_ignore_ascii_case(&b'i')
        && bytes[1].eq_ignore_ascii_case(&b's');
    let start = if starts_with_is { 2 } else { 0 };
    let mut out = String::with_capacity(bytes.len());
    for &b in &bytes[start..] {
        if b == b' ' || b == b'_' || b == b'-' {
            continue;
        } else if b.is_ascii_uppercase() {
            out.push((b + (b'a' - b'A')) as char);
        } else if b <= 0x7F {
            out.push(b as char);
        }
    }
    if starts_with_is && out == "c" {
        return "isc".to_string();
    }
    out
}

const UNICODE_GC_ALIASES: &[&str] = &[
    "any", "ascii", "assigned", "c", "casedletter", "cc", "cf", "closepunctuation", "cn",
    "cntrl", "co", "combiningmark", "connectorpunctuation", "control", "currencysymbol",
    "dashpunctuation", "decimalnumber", "digit", "enclosingmark", "finalpunctuation", "format",
    "initialpunctuation", "l", "lc", "letter", "letternumber", "lineseparator", "ll", "lm",
    "lo", "lowercaseletter", "lt", "lu", "m", "mark", "mathsymbol", "mc", "me", "mn",
    "modifierletter", "modifiersymbol", "n", "nd", "nl", "no", "nonspacingmark", "number",
    "openpunctuation", "other", "otherletter", "othernumber", "otherpunctuation", "othersymbol",
    "p", "paragraphseparator", "pc", "pd", "pe", "pf", "pi", "po", "privateuse", "ps", "punct",
    "punctuation", "s", "sc", "separator", "sk", "sm", "so", "spaceseparator", "spacingmark",
    "symbol", "titlecaseletter", "unassigned", "uppercaseletter", "z", "zl", "zp", "zs",
];

const UNICODE_BOOL_ALIASES: &[&str] = &[
    "ahex", "alpha", "alphabetic", "asciihexdigit", "bidic", "bidicontrol", "bidim",
    "bidimirrored", "cased", "caseignorable", "changeswhencasefolded", "changeswhencasemapped",
    "changeswhenlowercased", "changeswhentitlecased", "changeswhenuppercased", "ci", "cwcf",
    "cwcm", "cwl", "cwt", "cwu", "dash", "defaultignorablecodepoint", "dep", "deprecated", "di",
    "dia", "diacritic", "ebase", "ecomp", "emod", "emoji", "emojicomponent", "emojimodifier",
    "emojimodifierbase", "emojipresentation", "epres", "ext", "extendedpictographic",
    "extender", "extpict", "graphemebase", "graphemeextend", "graphemelink", "grbase", "grext",
    "grlink", "hex", "hexdigit", "hyphen", "idc", "idcompatmathcontinue", "idcompatmathstart",
    "idcontinue", "ideo", "ideographic", "ids", "idsb", "idsbinaryoperator", "idst", "idstart",
    "idstrinaryoperator", "idsu", "idsunaryoperator", "joinc", "joincontrol", "loe",
    "logicalorderexception", "lower", "lowercase", "math", "mcm", "modifiercombiningmark",
    "nchar", "noncharactercodepoint", "oalpha", "odi", "ogrext", "oidc", "oids", "olower",
    "omath", "otheralphabetic", "otherdefaultignorablecodepoint", "othergraphemeextend",
    "otheridcontinue", "otheridstart", "otherlowercase", "othermath", "otheruppercase",
    "oupper", "patsyn", "patternsyntax", "patternwhitespace", "patws", "pcm",
    "prependedconcatenationmark", "qmark", "quotationmark", "radical", "regionalindicator",
    "ri", "sd", "sentenceterminal", "softdotted", "space", "sterm", "term",
    "terminalpunctuation", "uideo", "unifiedideograph", "upper", "uppercase",
    "variationselector", "vs", "whitespace", "wspace", "xidc", "xidcontinue", "xids", "xidstart",
];

const UNICODE_SCRIPT_ALIASES: &[&str] = &[
    "adlam", "adlm", "aghb", "ahom", "anatolianhieroglyphs", "arab", "arabic", "armenian",
    "armi", "armn", "avestan", "avst", "bali", "balinese", "bamu", "bamum", "bass", "bassavah",
    "batak", "batk", "beng", "bengali", "bhaiksuki", "bhks", "bopo", "bopomofo", "brah",
    "brahmi", "brai", "braille", "bugi", "buginese", "buhd", "buhid", "cakm",
    "canadianaboriginal", "cans", "cari", "carian", "caucasianalbanian", "chakma", "cham",
    "cher", "cherokee", "chorasmian", "chrs", "common", "copt", "coptic", "cpmn", "cprt",
    "cuneiform", "cypriot", "cyprominoan", "cyrillic", "cyrl", "deseret", "deva", "devanagari",
    "diak", "divesakuru", "dogr", "dogra", "dsrt", "dupl", "duployan", "egyp",
    "egyptianhieroglyphs", "elba", "elbasan", "elym", "elymaic", "ethi", "ethiopic", "gara",
    "garay", "geor", "georgian", "glag", "glagolitic", "gong", "gonm", "goth", "gothic", "gran",
    "grantha", "greek", "grek", "gujarati", "gujr", "gukh", "gunjalagondi", "gurmukhi", "guru",
    "gurungkhema", "han", "hang", "hangul", "hani", "hanifirohingya", "hano", "hanunoo", "hatr",
    "hatran", "hebr", "hebrew", "hira", "hiragana", "hluw", "hmng", "hmnp", "hung",
    "imperialaramaic", "inherited", "inscriptionalpahlavi", "inscriptionalparthian", "ital",
    "java", "javanese", "kaithi", "kali", "kana", "kannada", "katakana", "kawi", "kayahli",
    "khar", "kharoshthi", "khitansmallscript", "khmer", "khmr", "khoj", "khojki", "khudawadi",
    "kiratrai", "kits", "knda", "krai", "kthi", "lana", "lao", "laoo", "latin", "latn", "lepc",
    "lepcha", "limb", "limbu", "lina", "linb", "lineara", "linearb", "lisu", "lyci", "lycian",
    "lydi", "lydian", "mahajani", "mahj", "maka", "makasar", "malayalam", "mand", "mandaic",
    "mani", "manichaean", "marc", "marchen", "masaramgondi", "medefaidrin", "medf",
    "meeteimayek", "mend", "mendekikakui", "merc", "mero", "meroiticcursive",
    "meroitichieroglyphs", "miao", "mlym", "modi", "mong", "mongolian", "mro", "mroo", "mtei",
    "mult", "multani", "myanmar", "mymr", "nabataean", "nagm", "nagmundari", "nand",
    "nandinagari", "narb", "nbat", "newa", "newtailue", "nko", "nkoo", "nshu", "nushu",
    "nyiakengpuachuehmong", "ogam", "ogham", "olchiki", "olck", "oldhungarian", "olditalic",
    "oldnortharabian", "oldpermic", "oldpersian", "oldsogdian", "oldsoutharabian", "oldturkic",
    "olduyghur", "olonal", "onao", "oriya", "orkh", "orya", "osage", "osge", "osma", "osmanya",
    "ougr", "pahawhhmong", "palm", "palmyrene", "pauc", "paucinhau", "perm", "phag", "phagspa",
    "phli", "phlp", "phnx", "phoenician", "plrd", "prti", "psalterpahlavi", "qaac", "qaai",
    "rejang", "rjng", "rohg", "runic", "runr", "samaritan", "samr", "sarb", "saur",
    "saurashtra", "sgnw", "sharada", "shavian", "shaw", "shrd", "sidd", "siddham",
    "signwriting", "sind", "sinh", "sinhala", "sogd", "sogdian", "sogo", "sora", "sorasompeng",
    "soyo", "soyombo", "sund", "sundanese", "sunu", "sunuwar", "sylo", "sylotinagri", "syrc",
    "syriac", "tagalog", "tagb", "tagbanwa", "taile", "taitham", "taiviet", "takr", "takri",
    "tale", "talu", "tamil", "taml", "tang", "tangsa", "tangut", "tavt", "telu", "telugu",
    "tfng", "tglg", "thaa", "thaana", "thai", "tibetan", "tibt", "tifinagh", "tirh", "tirhuta",
    "tnsa", "todhri", "todr", "toto", "tulutigalari", "tutg", "ugar", "ugaritic", "vai", "vaii",
    "vith", "vithkuqi", "wancho", "wara", "warangciti", "wcho", "xpeo", "xsux", "yezi",
    "yezidi", "yi", "yiii", "zanabazarsquare", "zanb", "zinh", "zyyy",
];

const UNICODE_AGE_ALIASES: &[&str] = &[
    "1.1", "10.0", "11.0", "12.0", "12.1", "13.0", "14.0", "15.0", "15.1", "16.0", "2.0", "2.1",
    "3.0", "3.1", "3.2", "4.0", "4.1", "5.0", "5.1", "5.2", "6.0", "6.1", "6.2", "6.3", "7.0",
    "8.0", "9.0", "v100", "v11", "v110", "v120", "v121", "v130", "v140", "v150", "v151", "v160",
    "v20", "v21", "v30", "v31", "v32", "v40", "v41", "v50", "v51", "v52", "v60", "v61", "v62",
    "v63", "v70", "v80", "v90",
];

const UNICODE_GCB_ALIASES: &[&str] = &[
    "cn", "control", "cr", "ex", "extend", "l", "lf", "lv", "lvt", "pp", "prepend",
    "regionalindicator", "ri", "sm", "spacingmark", "t", "v", "zwj",
];

const UNICODE_SB_ALIASES: &[&str] = &[
    "at", "aterm", "cl", "close", "cr", "ex", "extend", "fo", "format", "le", "lf", "lo",
    "lower", "nu", "numeric", "oletter", "sc", "scontinue", "se", "sep", "sp", "st", "sterm",
    "up", "upper",
];

const UNICODE_WB_ALIASES: &[&str] = &[
    "aletter", "cr", "doublequote", "dq", "ex", "extend", "extendnumlet", "fo", "format",
    "hebrewletter", "hl", "ka", "katakana", "le", "lf", "mb", "midletter", "midnum",
    "midnumlet", "ml", "mn", "newline", "nl", "nu", "numeric", "regionalindicator", "ri",
    "singlequote", "sq", "wsegspace", "zwj",
];

fn validate_unicode_property_spec(spec: &str, negated: bool) -> Result<u64, String> {
    if spec.is_empty() {
        return Err("Invalid regex: empty Unicode property '\\p{}'".to_string());
    }
    let split = if let Some(idx) = spec.find("!=") {
        Some((&spec[..idx], &spec[idx + 2..], true))
    } else if let Some(idx) = spec.find(':') {
        Some((&spec[..idx], &spec[idx + 1..], false))
    } else if let Some(idx) = spec.find('=') {
        Some((&spec[..idx], &spec[idx + 1..], false))
    } else {
        None
    };

    if let Some((raw_name, raw_val, op_neg)) = split {
        let name = symbolic_name_normalize(raw_name);
        let val = symbolic_name_normalize(raw_val);
        let table: &[&str] = match name.as_str() {
            "gc" | "generalcategory" => UNICODE_GC_ALIASES,
            "sc" | "script" | "scx" | "scriptextensions" => UNICODE_SCRIPT_ALIASES,
            "age" => UNICODE_AGE_ALIASES,
            "gcb" | "graphemeclusterbreak" => UNICODE_GCB_ALIASES,
            "sb" | "sentencebreak" => UNICODE_SB_ALIASES,
            "wb" | "wordbreak" => UNICODE_WB_ALIASES,
            _ => {
                return Err(format!(
                    "Invalid regex: Unicode property '{raw_name}' not found"
                ));
            }
        };
        if table.binary_search(&val.as_str()).is_err() {
            return Err(format!(
                "Invalid regex: Unicode property value '{raw_val}' not found for '{raw_name}'"
            ));
        }
        let is_neg = negated ^ op_neg;
        let cost = if is_neg {
            36_000
        } else if matches!(name.as_str(), "sc" | "script" | "scx" | "scriptextensions") {
            3_600
        } else if val == "ascii" {
            100
        } else if matches!(val.as_str(), "nd" | "decimalnumber" | "digit") {
            5_200
        } else {
            36_000
        };
        return Ok(cost);
    }

    let norm = symbolic_name_normalize(spec);
    let is_bool = norm != "cf"
        && norm != "sc"
        && norm != "lc"
        && UNICODE_BOOL_ALIASES.binary_search(&norm.as_str()).is_ok();
    let is_gc = UNICODE_GC_ALIASES.binary_search(&norm.as_str()).is_ok();
    let is_script = UNICODE_SCRIPT_ALIASES.binary_search(&norm.as_str()).is_ok();
    if is_bool || is_gc || is_script {
        let cost = if negated {
            36_000
        } else if norm == "ascii" {
            100
        } else if matches!(norm.as_str(), "nd" | "decimalnumber" | "digit") {
            5_200
        } else if matches!(norm.as_str(), "space" | "whitespace" | "wspace") {
            700
        } else if matches!(
            norm.as_str(),
            "emoji"
                | "epres"
                | "emojipresentation"
                | "extpict"
                | "extendedpictographic"
                | "emod"
                | "emojimodifier"
                | "ebase"
                | "emojimodifierbase"
                | "ecomp"
                | "emojicomponent"
        ) {
            9_000
        } else if is_script && !is_gc && !is_bool {
            3_600
        } else {
            36_000
        };
        Ok(cost)
    } else {
        Err(format!("Invalid regex: Unicode property '{spec}' not found"))
    }
}

#[derive(Debug, Clone, Copy)]
struct RegexFlags {
    case_insensitive: bool,
    unicode: bool,
    ignore_whitespace: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassAtom {
    Literal(char),
    Class(u64),
}

fn is_escapeable_character(c: char) -> bool {
    c.is_ascii() && !c.is_ascii_alphanumeric() && c != '<' && c != '>'
}

fn is_valid_posix_name(name: &str) -> bool {
    matches!(
        name,
        "alnum"
            | "alpha"
            | "ascii"
            | "blank"
            | "cntrl"
            | "digit"
            | "graph"
            | "lower"
            | "print"
            | "punct"
            | "space"
            | "upper"
            | "word"
            | "xdigit"
    )
}

fn skip_ws_if_x(chars: &[char], i: &mut usize, ignore_ws: bool) {
    if !ignore_ws {
        return;
    }
    while *i < chars.len() {
        match chars[*i] {
            ' ' | '\t' | '\n' | '\r' | '\x0C' | '\x0B' => *i += 1,
            '#' => {
                while *i < chars.len() && chars[*i] != '\n' {
                    *i += 1;
                }
                if *i < chars.len() && chars[*i] == '\n' {
                    *i += 1;
                }
            }
            _ => break,
        }
    }
}

fn parse_hex_escape(chars: &[char], kind_idx: usize) -> Result<(usize, char, bool), String> {
    let kind = chars[kind_idx];
    let fixed_len = match kind {
        'x' => 2,
        'u' => 4,
        'U' => 8,
        _ => unreachable!(),
    };
    let mut j = kind_idx + 1;
    let (hex, is_fixed_x): (String, bool) = if chars.get(j) == Some(&'{') {
        j += 1;
        let start = j;
        while j < chars.len() && chars[j] != '}' {
            if !chars[j].is_ascii_hexdigit() {
                return Err(format!("Invalid regex: invalid hex escape '\\{kind}{{...}}'"));
            }
            j += 1;
        }
        if j >= chars.len() || j == start {
            return Err(format!("Invalid regex: malformed hex escape '\\{kind}{{...}}'"));
        }
        let s: String = chars[start..j].iter().collect();
        j += 1;
        (s, false)
    } else {
        if j + fixed_len > chars.len() {
            return Err(format!("Invalid regex: truncated hex escape '\\{kind}'"));
        }
        let slice = &chars[j..j + fixed_len];
        if !slice.iter().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("Invalid regex: non-hex digit in '\\{kind}' escape"));
        }
        j += fixed_len;
        (slice.iter().collect(), kind == 'x')
    };
    let trimmed_hex = hex.trim_start_matches('0');
    let code = if trimmed_hex.is_empty() {
        0u32
    } else {
        u32::from_str_radix(trimmed_hex, 16)
            .map_err(|_| format!("Invalid regex: out-of-range hex escape '\\{kind}'"))?
    };
    let ch = char::from_u32(code)
        .ok_or_else(|| format!("Invalid regex: invalid Unicode scalar 0x{hex}"))?;
    Ok((j, ch, is_fixed_x))
}

fn parse_unicode_property(
    chars: &[char],
    start_idx: usize,
    negated: bool,
    flags: RegexFlags,
) -> Result<(usize, u64), String> {
    if !flags.unicode {
        return Err(
            "Invalid regex: Unicode character class not allowed when unicode flag is disabled"
                .to_string(),
        );
    }
    let mut j = start_idx;
    skip_ws_if_x(chars, &mut j, flags.ignore_whitespace);
    let Some(&ch) = chars.get(j) else {
        return Err("Invalid regex: truncated Unicode property escape '\\p'".to_string());
    };
    if ch == '{' {
        j += 1;
        let mut spec = String::new();
        loop {
            skip_ws_if_x(chars, &mut j, flags.ignore_whitespace);
            let Some(&c) = chars.get(j) else {
                return Err("Invalid regex: unclosed Unicode property '\\p{'".to_string());
            };
            if c == '}' {
                j += 1;
                break;
            }
            spec.push(c);
            j += 1;
        }
        let cost = validate_unicode_property_spec(&spec, negated)?;
        Ok((j, cost))
    } else if ch == '\\' {
        Err("Invalid regex: invalid Unicode property escape '\\p\\'".to_string())
    } else {
        let mut buf = [0u8; 4];
        let cost = validate_unicode_property_spec(ch.encode_utf8(&mut buf), negated)?;
        Ok((j + 1, cost))
    }
}

fn maybe_parse_ascii_class(chars: &[char], start_idx: usize) -> Option<(usize, bool)> {
    if chars.get(start_idx) != Some(&'[') || chars.get(start_idx + 1) != Some(&':') {
        return None;
    }
    let mut j = start_idx + 2;
    let negated = if chars.get(j) == Some(&'^') {
        j += 1;
        true
    } else {
        false
    };
    let name_start = j;
    while j < chars.len() && chars[j] != ':' {
        j += 1;
    }
    if j + 1 >= chars.len() || chars[j] != ':' || chars[j + 1] != ']' {
        return None;
    }
    let name: String = chars[name_start..j].iter().collect();
    if !is_valid_posix_name(&name) {
        return None;
    }
    Some((j + 2, negated))
}

fn parse_escape_in_class(
    chars: &[char],
    slash_idx: usize,
    flags: RegexFlags,
) -> Result<(usize, ClassAtom), String> {
    let Some(&next) = chars.get(slash_idx + 1) else {
        return Err("Invalid regex: trailing unescaped backslash '\\'".to_string());
    };
    match next {
        'w' => Ok((slash_idx + 2, ClassAtom::Class(if flags.unicode { 36_000 } else { 100 }))),
        'd' => Ok((slash_idx + 2, ClassAtom::Class(if flags.unicode { 5_200 } else { 100 }))),
        's' => Ok((slash_idx + 2, ClassAtom::Class(if flags.unicode { 700 } else { 100 }))),
        'W' => {
            if !flags.unicode {
                return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
            }
            Ok((slash_idx + 2, ClassAtom::Class(36_000)))
        }
        'D' => {
            if !flags.unicode {
                return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
            }
            Ok((slash_idx + 2, ClassAtom::Class(5_200)))
        }
        'S' => {
            if !flags.unicode {
                return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
            }
            Ok((slash_idx + 2, ClassAtom::Class(700)))
        }
        'p' | 'P' => {
            let (end, cost) = parse_unicode_property(chars, slash_idx + 2, next == 'P', flags)?;
            Ok((end, ClassAtom::Class(cost)))
        }
        'x' | 'u' | 'U' => {
            let (end, ch, is_fixed_x) = parse_hex_escape(chars, slash_idx + 1)?;
            if !flags.unicode {
                if is_fixed_x && (ch as u32) > 0x7F {
                    return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                }
                if !ch.is_ascii() {
                    return Err(
                        "Invalid regex: Unicode literal not allowed in byte class".to_string()
                    );
                }
            }
            Ok((end, ClassAtom::Literal(ch)))
        }
        'n' => Ok((slash_idx + 2, ClassAtom::Literal('\n'))),
        'r' => Ok((slash_idx + 2, ClassAtom::Literal('\r'))),
        't' => Ok((slash_idx + 2, ClassAtom::Literal('\t'))),
        'f' => Ok((slash_idx + 2, ClassAtom::Literal('\x0C'))),
        'v' => Ok((slash_idx + 2, ClassAtom::Literal('\x0B'))),
        'a' => Ok((slash_idx + 2, ClassAtom::Literal('\x07'))),
        '0'..='9' => Err(format!(
            "Invalid regex: octal or backreference escape '\\{next}' is not supported"
        )),
        'b' | 'B' | 'A' | 'z' | '<' | '>' => Err(format!(
            "Invalid regex: assertion escape '\\{next}' is not allowed inside character class"
        )),
        other if is_escapeable_character(other) => Ok((slash_idx + 2, ClassAtom::Literal(other))),
        other => Err(format!(
            "Invalid regex: unrecognized escape sequence '\\{other}' inside character class"
        )),
    }
}

fn parse_class_item(
    chars: &[char],
    i: usize,
    flags: RegexFlags,
) -> Result<(usize, ClassAtom), String> {
    let Some(&ch) = chars.get(i) else {
        return Err("Invalid regex: unclosed character class '['".to_string());
    };
    if ch == '\\' {
        parse_escape_in_class(chars, i, flags)
    } else {
        if !flags.unicode && !ch.is_ascii() {
            return Err("Invalid regex: Unicode literal not allowed in byte class".to_string());
        }
        Ok((i + 1, ClassAtom::Literal(ch)))
    }
}

fn parse_char_class(
    chars: &[char],
    start_idx: usize,
    flags: RegexFlags,
    depth: usize,
) -> Result<(usize, u64), String> {
    const NEST_LIMIT: usize = 250;
    if depth > NEST_LIMIT {
        return Err("Invalid regex: character class nesting limit exceeded".to_string());
    }
    let mut i = start_idx + 1;
    skip_ws_if_x(chars, &mut i, flags.ignore_whitespace);
    let negated = if chars.get(i) == Some(&'^') {
        i += 1;
        skip_ws_if_x(chars, &mut i, flags.ignore_whitespace);
        true
    } else {
        false
    };
    if !flags.unicode && negated {
        return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
    }
    let mut saw_leading_hyphen = false;
    while chars.get(i) == Some(&'-') {
        saw_leading_hyphen = true;
        i += 1;
        skip_ws_if_x(chars, &mut i, flags.ignore_whitespace);
    }
    if !saw_leading_hyphen && chars.get(i) == Some(&']') {
        i += 1;
    }

    let mut cost: u64 = 64;
    while i < chars.len() {
        skip_ws_if_x(chars, &mut i, flags.ignore_whitespace);
        let Some(&ch) = chars.get(i) else {
            return Err("Invalid regex: unclosed character class '['".to_string());
        };
        if ch == ']' {
            return Ok((i + 1, cost));
        }
        if ch == '[' {
            if let Some((next_i, posix_negated)) = maybe_parse_ascii_class(chars, i) {
                if !flags.unicode && posix_negated {
                    return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                }
                i = next_i;
                cost = cost.saturating_add(32);
                continue;
            }
            let (next_i, sub_cost) = parse_char_class(chars, i, flags, depth + 1)?;
            i = next_i;
            cost = cost.saturating_add(sub_cost);
            continue;
        }
        if (ch == '&' || ch == '-' || ch == '~') && chars.get(i + 1) == Some(&ch) {
            i += 2;
            continue;
        }
        let (next_i, prim1) = parse_class_item(chars, i, flags)?;
        i = next_i;
        if let ClassAtom::Class(cls_cost) = prim1 {
            cost = cost.max(cls_cost).saturating_add(cost.min(cls_cost) / 4);
        }
        skip_ws_if_x(chars, &mut i, flags.ignore_whitespace);
        if chars.get(i) == Some(&'-') {
            let mut peek_i = i + 1;
            skip_ws_if_x(chars, &mut peek_i, flags.ignore_whitespace);
            let peek_ch = chars.get(peek_i).copied();
            if peek_ch != Some(']') && peek_ch != Some('-') {
                let ClassAtom::Literal(start_ch) = prim1 else {
                    return Err("Invalid regex: character class cannot start a range".to_string());
                };
                i = peek_i;
                if i >= chars.len() {
                    return Err("Invalid regex: unclosed character class '['".to_string());
                }
                let (end_i, prim2) = parse_class_item(chars, i, flags)?;
                let ClassAtom::Literal(end_ch) = prim2 else {
                    return Err("Invalid regex: character class cannot end a range".to_string());
                };
                if start_ch > end_ch {
                    return Err(format!(
                        "Invalid regex: reversed character class range '{start_ch}-{end_ch}'"
                    ));
                }
                i = end_i;
            }
        }
    }
    Err("Invalid regex: unclosed character class '['".to_string())
}

fn maybe_parse_special_word_boundary(
    chars: &[char],
    brace_idx: usize,
    flags: RegexFlags,
) -> Result<Option<usize>, String> {
    let mut j = brace_idx + 1;
    skip_ws_if_x(chars, &mut j, flags.ignore_whitespace);
    let Some(&first) = chars.get(j) else {
        return Err("Invalid regex: unclosed '\\b{'".to_string());
    };
    if !(first.is_ascii_alphabetic() || first == '-') {
        return Ok(None);
    }
    let mut name = String::new();
    while j < chars.len() {
        let c = chars[j];
        if c.is_ascii_alphabetic() || c == '-' {
            name.push(c);
            j += 1;
            skip_ws_if_x(chars, &mut j, flags.ignore_whitespace);
        } else {
            break;
        }
    }
    if chars.get(j) != Some(&'}') {
        return Err("Invalid regex: unclosed special word boundary '\\b{...}'".to_string());
    }
    if !matches!(name.as_str(), "start" | "end" | "start-half" | "end-half") {
        return Err(format!(
            "Invalid regex: unrecognized word boundary '\\b{{{name}}}'"
        ));
    }
    Ok(Some(j + 1))
}

fn literal_cost(ch: char, flags: RegexFlags) -> u64 {
    if ch.is_ascii() {
        if flags.case_insensitive && ch.is_ascii_alphabetic() {
            80
        } else {
            32
        }
    } else {
        let base = 32 * (ch.len_utf8() as u64);
        if flags.case_insensitive && flags.unicode && ch.is_alphabetic() {
            base + 32
        } else {
            base
        }
    }
}

/// Synchronous Rust `regex` syntax & bounded NFA expansion guard for the AutoMod editor,
/// aligned with `AutoModEngine`'s `regex::RegexBuilder` (`case_insensitive(true)`,
/// `utf8(true)`, `size_limit = 64 KiB`, `dfa_size_limit = 64 KiB`).
pub fn validate_automod_regex_syntax(pattern: &str) -> Result<(), String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.len() > 2048 {
        return Err("Regex pattern exceeds maximum length (2048 chars)".to_string());
    }

    const MAX_NFA_SIZE_BYTES: u64 = 64 * 1024;
    const NEST_LIMIT: usize = 250;

    #[derive(Debug, Clone, Copy)]
    struct GroupFrame {
        saved_flags: RegexFlags,
        cost: u64,
        is_capture: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum QuantState {
        None,
        Atom(u64),
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut flags = RegexFlags {
        case_insensitive: true,
        unicode: true,
        ignore_whitespace: false,
    };
    let mut frames: Vec<GroupFrame> = Vec::new();
    let mut seen_group_names = std::collections::HashSet::new();
    let mut total_cost: u64 = 256;
    let mut quant = QuantState::None;
    let mut rep_chain: usize = 0;
    let mut i = 0usize;

    let add_cost = |frames: &mut Vec<GroupFrame>, total_cost: &mut u64, delta: u64| -> Result<(), String> {
        *total_cost = total_cost.saturating_add(delta);
        if let Some(top) = frames.last_mut() {
            top.cost = top.cost.saturating_add(delta);
        }
        if *total_cost > MAX_NFA_SIZE_BYTES {
            return Err(
                "Invalid regex: compiled pattern exceeds AutoMod 64 KiB size limit".to_string(),
            );
        }
        Ok(())
    };

    while i < chars.len() {
        skip_ws_if_x(&chars, &mut i, flags.ignore_whitespace);
        if i >= chars.len() {
            break;
        }
        let ch = chars[i];
        match ch {
            '\\' => {
                let Some(&next) = chars.get(i + 1) else {
                    return Err("Invalid regex: trailing unescaped backslash '\\'".to_string());
                };
                let cost = match next {
                    'a' | 'f' | 't' | 'n' | 'r' | 'v' | 'A' | 'z' | 'B' | '<' | '>' => {
                        i += 2;
                        32
                    }
                    'b' => {
                        i += 2;
                        if chars.get(i) == Some(&'{') {
                            if let Some(end) = maybe_parse_special_word_boundary(&chars, i, flags)? {
                                i = end;
                            }
                        }
                        32
                    }
                    'w' => {
                        i += 2;
                        if flags.unicode { 36_000 } else { 100 }
                    }
                    'd' => {
                        i += 2;
                        if flags.unicode { 5_200 } else { 100 }
                    }
                    's' => {
                        i += 2;
                        if flags.unicode { 700 } else { 100 }
                    }
                    'W' => {
                        if !flags.unicode {
                            return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                        }
                        i += 2;
                        36_000
                    }
                    'D' => {
                        if !flags.unicode {
                            return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                        }
                        i += 2;
                        5_200
                    }
                    'S' => {
                        if !flags.unicode {
                            return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                        }
                        i += 2;
                        700
                    }
                    'p' | 'P' => {
                        let (next_i, cost) =
                            parse_unicode_property(&chars, i + 2, next == 'P', flags)?;
                        i = next_i;
                        cost
                    }
                    'x' | 'u' | 'U' => {
                        let (next_i, c, is_fixed_x) = parse_hex_escape(&chars, i + 1)?;
                        if !flags.unicode && is_fixed_x && (c as u32) > 0x7F {
                            return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                        }
                        i = next_i;
                        literal_cost(c, flags)
                    }
                    '0'..='9' => {
                        return Err(format!(
                            "Invalid regex: backreference '\\{next}' is not supported by Rust regex"
                        ));
                    }
                    c if is_escapeable_character(c) => {
                        i += 2;
                        literal_cost(c, flags)
                    }
                    other => {
                        return Err(format!(
                            "Invalid regex: unrecognized escape sequence '\\{other}'"
                        ));
                    }
                };
                add_cost(&mut frames, &mut total_cost, cost)?;
                quant = QuantState::Atom(cost);
                rep_chain = 0;
            }
            '[' => {
                let (next_i, cost) = parse_char_class(&chars, i, flags, 0)?;
                i = next_i;
                add_cost(&mut frames, &mut total_cost, cost)?;
                quant = QuantState::Atom(cost);
                rep_chain = 0;
            }
            '.' => {
                if !flags.unicode {
                    return Err("Invalid regex: pattern can match invalid UTF-8".to_string());
                }
                i += 1;
                add_cost(&mut frames, &mut total_cost, 960)?;
                quant = QuantState::Atom(960);
                rep_chain = 0;
            }
            '(' => {
                if frames.len() >= NEST_LIMIT {
                    return Err("Invalid regex: group nesting limit exceeded".to_string());
                }
                if chars.get(i + 1) == Some(&'?') {
                    let mut j = i + 2;
                    if j >= chars.len() {
                        return Err(format!(
                            "Invalid regex: unclosed group modifier '(?' at position {i}"
                        ));
                    }
                    if chars[j] == ':' {
                        frames.push(GroupFrame { saved_flags: flags, cost: 0, is_capture: false });
                        i = j + 1;
                        quant = QuantState::None;
                        rep_chain = 0;
                        continue;
                    }
                    if chars[j] == 'P' || chars[j] == '<' {
                        if chars[j] == 'P' {
                            j += 1;
                            if chars.get(j) != Some(&'<') {
                                return Err(format!(
                                    "Invalid regex: unsupported '(?P' construct at position {i}"
                                ));
                            }
                        }
                        j += 1;
                        let name_start = j;
                        while j < chars.len() && chars[j] != '>' {
                            j += 1;
                        }
                        if j >= chars.len() || chars[j] != '>' {
                            return Err(format!(
                                "Invalid regex: unclosed named capture group at position {i}"
                            ));
                        }
                        let name: String = chars[name_start..j].iter().collect();
                        let mut nchars = name.chars();
                        let Some(first) = nchars.next() else {
                            return Err("Invalid regex: empty capture group name".to_string());
                        };
                        if !(first.is_ascii_alphabetic() || first == '_')
                            || !nchars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                        {
                            return Err(format!(
                                "Invalid regex: invalid capture group name '{name}'"
                            ));
                        }
                        if !seen_group_names.insert(name.clone()) {
                            return Err(format!(
                                "Invalid regex: duplicate capture group name '{name}'"
                            ));
                        }
                        frames.push(GroupFrame { saved_flags: flags, cost: 0, is_capture: true });
                        i = j + 1;
                        quant = QuantState::None;
                        rep_chain = 0;
                        continue;
                    }
                    let mut seen_flags = [false; 7];
                    let mut any_flag = false;
                    let mut saw_neg = false;
                    let mut any_neg_flag = false;
                    let mut new_flags = flags;
                    while j < chars.len() && chars[j] != ')' && chars[j] != ':' {
                        let fch = chars[j];
                        if fch == '-' {
                            if saw_neg {
                                return Err(format!(
                                    "Invalid regex: duplicate '-' in flag group at position {i}"
                                ));
                            }
                            saw_neg = true;
                            j += 1;
                            continue;
                        }
                        let flag_idx = match fch {
                            'i' => {
                                new_flags.case_insensitive = !saw_neg;
                                0
                            }
                            'm' => 1,
                            's' => 2,
                            'R' => 3,
                            'U' => 4,
                            'u' => {
                                new_flags.unicode = !saw_neg;
                                5
                            }
                            'x' => {
                                new_flags.ignore_whitespace = !saw_neg;
                                6
                            }
                            _ => {
                                return Err(format!(
                                    "Invalid regex: unrecognized flag '{fch}' in '(?' at position {i}"
                                ));
                            }
                        };
                        if seen_flags[flag_idx] {
                            return Err(format!(
                                "Invalid regex: duplicate flag '{fch}' in '(?' at position {i}"
                            ));
                        }
                        seen_flags[flag_idx] = true;
                        any_flag = true;
                        if saw_neg {
                            any_neg_flag = true;
                        }
                        j += 1;
                    }
                    if !any_flag || (saw_neg && !any_neg_flag) {
                        return Err(format!(
                            "Invalid regex: empty or incomplete flag modifier '(?' at position {i}"
                        ));
                    }
                    match chars.get(j).copied() {
                        Some(')') => {
                            flags = new_flags;
                            i = j + 1;
                            quant = QuantState::None;
                            rep_chain = 0;
                            continue;
                        }
                        Some(':') => {
                            frames.push(GroupFrame {
                                saved_flags: flags,
                                cost: 0,
                                is_capture: false,
                            });
                            flags = new_flags;
                            i = j + 1;
                            quant = QuantState::None;
                            rep_chain = 0;
                            continue;
                        }
                        _ => {
                            return Err(format!(
                                "Invalid regex: unclosed flag group '(?' at position {i}"
                            ));
                        }
                    }
                } else {
                    frames.push(GroupFrame { saved_flags: flags, cost: 0, is_capture: true });
                    i += 1;
                    quant = QuantState::None;
                    rep_chain = 0;
                }
            }
            ')' => {
                let Some(frame) = frames.pop() else {
                    return Err("Invalid regex: unmatched closing ')'".to_string());
                };
                flags = frame.saved_flags;
                let cap_overhead = if frame.is_capture { 64 } else { 0 };
                total_cost = total_cost.saturating_add(cap_overhead);
                if total_cost > MAX_NFA_SIZE_BYTES {
                    return Err(
                        "Invalid regex: compiled pattern exceeds AutoMod 64 KiB size limit"
                            .to_string(),
                    );
                }
                let group_total = frame.cost.saturating_add(cap_overhead);
                if let Some(parent) = frames.last_mut() {
                    parent.cost = parent.cost.saturating_add(group_total);
                }
                i += 1;
                quant = QuantState::Atom(group_total.max(32));
                rep_chain = 0;
            }
            '{' => {
                let QuantState::Atom(atom_cost) = quant else {
                    return Err(
                        "Invalid regex: repetition quantifier '{' without target".to_string(),
                    );
                };
                rep_chain += 1;
                if rep_chain > NEST_LIMIT {
                    return Err("Invalid regex: repetition nesting limit exceeded".to_string());
                }
                let mut j = i + 1;
                skip_ws_if_x(&chars, &mut j, flags.ignore_whitespace);
                let mut min_str = String::new();
                while j < chars.len() && chars[j].is_ascii_digit() {
                    min_str.push(chars[j]);
                    j += 1;
                    skip_ws_if_x(&chars, &mut j, flags.ignore_whitespace);
                }
                if min_str.is_empty() {
                    return Err("Invalid regex: missing repetition minimum in '{...}'".to_string());
                }
                let min_v = min_str
                    .parse::<u32>()
                    .map_err(|_| format!("Invalid regex: invalid repetition '{{{min_str}}}'"))?;
                let max_v = if chars.get(j) == Some(&',') {
                    j += 1;
                    skip_ws_if_x(&chars, &mut j, flags.ignore_whitespace);
                    if chars.get(j) == Some(&'}') {
                        None
                    } else {
                        let mut max_str = String::new();
                        while j < chars.len() && chars[j].is_ascii_digit() {
                            max_str.push(chars[j]);
                            j += 1;
                            skip_ws_if_x(&chars, &mut j, flags.ignore_whitespace);
                        }
                        if max_str.is_empty() {
                            return Err(
                                "Invalid regex: invalid repetition upper bound in '{...}'"
                                    .to_string(),
                            );
                        }
                        let mv = max_str.parse::<u32>().map_err(|_| {
                            format!("Invalid regex: invalid repetition bound '{max_str}'")
                        })?;
                        if mv < min_v {
                            return Err(format!(
                                "Invalid regex: repetition min > max in '{{{min_v},{mv}}}'"
                            ));
                        }
                        Some(mv)
                    }
                } else {
                    Some(min_v)
                };
                if chars.get(j) != Some(&'}') {
                    return Err("Invalid regex: unclosed or malformed repetition '{'".to_string());
                }
                i = j + 1;
                let mut q_idx = i;
                skip_ws_if_x(&chars, &mut q_idx, flags.ignore_whitespace);
                if chars.get(q_idx) == Some(&'?') {
                    i = q_idx + 1;
                }
                let bound = max_v.unwrap_or(min_v);
                if bound == 0 {
                    if max_v == Some(0) {
                        total_cost = total_cost.saturating_sub(atom_cost);
                        if let Some(top) = frames.last_mut() {
                            top.cost = top.cost.saturating_sub(atom_cost);
                        }
                        quant = QuantState::Atom(32);
                    } else {
                        add_cost(&mut frames, &mut total_cost, 40)?;
                        quant = QuantState::Atom(atom_cost.saturating_add(40));
                    }
                } else {
                    let optional_extra = max_v.map_or(1u64, |m| u64::from(m.saturating_sub(min_v)));
                    let total_rep = atom_cost
                        .saturating_mul(u64::from(bound))
                        .saturating_add(optional_extra.saturating_mul(40));
                    let extra = total_rep.saturating_sub(atom_cost);
                    add_cost(&mut frames, &mut total_cost, extra)?;
                    quant = QuantState::Atom(total_rep.max(32));
                }
            }
            '*' | '+' | '?' => {
                let QuantState::Atom(atom_cost) = quant else {
                    return Err(format!(
                        "Invalid regex: dangling quantifier '{ch}' at position {i}"
                    ));
                };
                rep_chain += 1;
                if rep_chain > NEST_LIMIT {
                    return Err("Invalid regex: repetition nesting limit exceeded".to_string());
                }
                i += 1;
                if chars.get(i) == Some(&'?') {
                    i += 1;
                }
                add_cost(&mut frames, &mut total_cost, 40)?;
                quant = QuantState::Atom(atom_cost.saturating_add(40));
            }
            '|' => {
                add_cost(&mut frames, &mut total_cost, 40)?;
                i += 1;
                quant = QuantState::None;
                rep_chain = 0;
            }
            _ => {
                let cost = literal_cost(ch, flags);
                add_cost(&mut frames, &mut total_cost, cost)?;
                i += 1;
                quant = QuantState::Atom(cost);
                rep_chain = 0;
            }
        }
    }

    if !frames.is_empty() {
        return Err("Invalid regex: unclosed group '('".to_string());
    }
    Ok(())
}
