//! Digits written out as words, because engines skip some numbers ("1950")
//! and misread others. English and Spanish are done here; every other language
//! goes through the `numbertext` crate, with `num2words2-core` for the Indic,
//! Arabic and Swahili ones it lacks.

#[derive(Clone, PartialEq)]
enum Lang {
  En,
  Es,
  /// Any other language, by its two-letter code.
  Other(String),
}

/// The 4-digit numbers read as years; anything else is a quantity.
const YEARS: std::ops::RangeInclusive<u64> = 1000..=2200;

/// Languages whose numbers `numbertext` can say (its year rule included).
const NUMBERTEXT_LANGS: &[&str] = &[
  "bg", "ca", "cs", "da", "de", "el", "et", "fi", "fr", "hr", "hu", "id", "it", "ja", "ko", "lt",
  "lv", "mr", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi", "zh",
];
/// Languages only `num2words2-core` knows.
const NUM2WORDS_LANGS: &[&str] = &["ar", "bn", "gu", "hi", "kn", "pa", "sw", "ta", "te"];
/// Write "1.234.567" for a million; the rest use "," or a space.
const DOT_THOUSANDS: &[&str] = &[
  "ca", "da", "de", "el", "es", "hr", "id", "it", "nl", "pt", "ro", "sl", "tr",
];

fn lang_of(language: &str) -> Option<Lang> {
  let l = language.trim().to_ascii_lowercase();
  let code = l.split(['-', '_']).next().unwrap_or("");
  match code {
    "en" => Some(Lang::En),
    "es" => Some(Lang::Es),
    c if NUMBERTEXT_LANGS.contains(&c) || NUM2WORDS_LANGS.contains(&c) => {
      Some(Lang::Other(c.to_string()))
    }
    _ => None,
  }
}

/// First code point of each script's own digits 0-9 (Arabic-Indic, Persian,
/// Devanagari, Bengali, Gurmukhi, Gujarati, Tamil, Telugu, Kannada, full-width).
const DIGIT_ZEROS: [u32; 10] = [
  0x0660, 0x06F0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0BE6, 0x0C66, 0x0CE6, 0xFF10,
];

/// The ASCII digit for one written in another script; any other char as is.
fn ascii_digit(c: char) -> char {
  if c.is_ascii() {
    return c;
  }
  for zero in DIGIT_ZEROS {
    let offset = (c as u32).wrapping_sub(zero);
    if offset < 10 {
      return char::from(b'0' + offset as u8);
    }
  }
  c
}

/// `text` with every standalone number spelled out in `language`.
pub fn expand(text: &str, language: &str) -> String {
  let Some(lang) = lang_of(language) else {
    return text.to_string();
  };
  // Chinese, Japanese and Korean write a counter straight after the digits
  // ("1950年"), so letters next to a number mean nothing there.
  let counters = matches!(&lang, Lang::Other(c) if ["zh", "ja", "ko"].contains(&c.as_str()));
  let chars: Vec<char> = text.chars().map(ascii_digit).collect();
  let mut out = String::with_capacity(text.len() + 16);
  let mut i = 0;
  while i < chars.len() {
    if !chars[i].is_ascii_digit() {
      out.push(chars[i]);
      i += 1;
      continue;
    }
    let start = i;
    let (mut end, mut spoken) = read_number(&chars, i, &lang);
    let mut suffixed = false;
    // Digits fused with letters ("mp3", "H2O", "3D") are a name, not a
    // quantity; a number the crate cannot say stays as written.
    let glued_before = !counters && start > 0 && chars[start - 1].is_alphabetic();
    let glued_after = !counters && chars.get(end).is_some_and(|c| c.is_alphabetic());
    if glued_after && !glued_before && lang == Lang::En {
      if let Some((suffix_end, words)) = en_suffixed(&chars, start, end) {
        end = suffix_end;
        spoken = words;
        suffixed = true;
      }
    }
    let glued = glued_before || (glued_after && !suffixed);
    if glued || spoken.trim().is_empty() {
      out.extend(&chars[start..end]);
      i = end;
      continue;
    }
    out.push_str(&spoken);
    i = end;
    // "1950-1960" and "12/05/1999": the separator is dropped later, so keep
    // the spoken numbers apart.
    if matches!(chars.get(i), Some('-' | '/')) && is_digit_at(&chars, i + 1) {
      out.push(' ');
      i += 1;
    }
  }
  out
}

/// "21st" -> "twenty first", "1950s" -> "nineteen fifties", "90s" -> "nineties":
/// the number at `start..end` with the English suffix that follows it.
fn en_suffixed(chars: &[char], start: usize, end: usize) -> Option<(usize, String)> {
  let mut suffix_end = end;
  while chars
    .get(suffix_end)
    .is_some_and(|c| c.is_ascii_alphabetic())
  {
    suffix_end += 1;
  }
  // A suffix followed by more letters is a word, not a suffix.
  if chars.get(suffix_end).is_some_and(|c| c.is_alphabetic()) {
    return None;
  }
  let digits: String = chars[start..end].iter().collect();
  if digits.len() > 12 || !digits.chars().all(|c| c.is_ascii_digit()) {
    return None;
  }
  let n: u64 = digits.parse().ok()?;
  let suffix: String = chars[end..suffix_end]
    .iter()
    .collect::<String>()
    .to_ascii_lowercase();
  match suffix.as_str() {
    "st" | "nd" | "rd" | "th" => Some((suffix_end, en_ordinal(&en_cardinal(n)))),
    "s" if n % 10 == 0 && n >= 10 => {
      let is_year = digits.len() == 4 && YEARS.contains(&n);
      if is_year {
        Some((suffix_end, en_plural(&en_year(n))))
      } else if n < 100 {
        Some((suffix_end, en_plural(&en_cardinal(n))))
      } else {
        None
      }
    }
    _ => None,
  }
}

/// Turn the last word of a spelled-out number into its ordinal.
fn en_ordinal(words: &str) -> String {
  let (head, last) = words.rsplit_once(' ').map_or(("", words), |(h, l)| (h, l));
  let ord = match last {
    "one" => "first".to_string(),
    "two" => "second".to_string(),
    "three" => "third".to_string(),
    "five" => "fifth".to_string(),
    "eight" => "eighth".to_string(),
    "nine" => "ninth".to_string(),
    "twelve" => "twelfth".to_string(),
    w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
    w => format!("{w}th"),
  };
  if head.is_empty() {
    ord
  } else {
    format!("{head} {ord}")
  }
}

/// Plural of the last word of a decade: "fifty" -> "fifties", "hundred" -> "hundreds".
fn en_plural(words: &str) -> String {
  match words.strip_suffix('y') {
    Some(stem) => format!("{stem}ies"),
    None => format!("{words}s"),
  }
}

fn is_digit_at(chars: &[char], i: usize) -> bool {
  chars.get(i).is_some_and(|c| c.is_ascii_digit())
}

/// Parse the number starting at `start`; returns where it ends and how it is said.
fn read_number(chars: &[char], start: usize, lang: &Lang) -> (usize, String) {
  let dot_thousands = match lang {
    Lang::En => false,
    Lang::Es => true,
    Lang::Other(c) => DOT_THOUSANDS.contains(&c.as_str()),
  };
  let thousands_sep = if dot_thousands { '.' } else { ',' };
  let mut i = start;
  while is_digit_at(chars, i) {
    i += 1;
  }
  let mut int_digits: String = chars[start..i].iter().collect();

  // "1,000,000" / "1.000.000": groups of exactly three digits.
  if i - start <= 3 && chars[start] != '0' {
    let mut j = i;
    let mut grouped = int_digits.clone();
    while chars.get(j) == Some(&thousands_sep)
      && (1..=3).all(|k| is_digit_at(chars, j + k))
      && !is_digit_at(chars, j + 4)
    {
      grouped.extend(&chars[j + 1..j + 4]);
      j += 4;
    }
    if j > i {
      int_digits = grouped;
      i = j;
    }
  }

  // Decimal part: the mark that is not the thousands one (a dot is common
  // even where the comma is the rule).
  let dec_seps: &[char] = if *lang == Lang::En {
    &['.']
  } else if dot_thousands {
    &[',', '.']
  } else {
    &['.', ',']
  };
  if chars.get(i).is_some_and(|c| dec_seps.contains(c)) && is_digit_at(chars, i + 1) {
    let mut j = i + 1;
    while is_digit_at(chars, j) {
      j += 1;
    }
    let frac: String = chars[i + 1..j].iter().collect();
    return (j, decimal_words(&int_digits, &frac, lang));
  }

  let is_plain = i - start == int_digits.len();
  (i, integer_words(&int_digits, lang, is_plain))
}

fn decimal_words(int_digits: &str, frac: &str, lang: &Lang) -> String {
  match lang {
    Lang::En => {
      format!(
        "{} point {}",
        integer_words(int_digits, lang, false),
        digit_words(frac, lang)
      )
    }
    Lang::Es => {
      format!(
        "{} coma {}",
        integer_words(int_digits, lang, false),
        digit_words(frac, lang)
      )
    }
    // numbertext knows each language's decimal wording, except Croatian,
    // which reads "3.14" as the integer 314.
    Lang::Other(c) if NUMBERTEXT_LANGS.contains(&c.as_str()) && c != "hr" => {
      numbertext::cardinal(c, &format!("{int_digits}.{frac}")).unwrap_or_default()
    }
    Lang::Other(_) => {
      format!(
        "{} {}",
        integer_words(int_digits, lang, false),
        digit_words(frac, lang)
      )
    }
  }
}

fn integer_words(digits: &str, lang: &Lang, may_be_year: bool) -> String {
  // "007", or a number too long to say sensibly (a card, a phone): one by one.
  // A two-digit "05" is a day or a minute, so it is just five.
  if (digits.len() > 2 && digits.starts_with('0')) || digits.len() > 12 {
    return digit_words(digits, lang);
  }
  let n: u64 = digits.parse().unwrap_or(0);
  let is_year = may_be_year && digits.len() == 4 && YEARS.contains(&n);
  match lang {
    Lang::En if is_year => en_year(n),
    Lang::En => en_cardinal(n),
    Lang::Es => es_cardinal(n),
    Lang::Other(c) => other_words(c, n, is_year),
  }
}

fn other_words(code: &str, n: u64, is_year: bool) -> String {
  if NUMBERTEXT_LANGS.contains(&code) {
    // Chinese years are read digit by digit: 1950 -> 一九五零.
    if is_year && code == "zh" {
      return digit_words(&n.to_string(), &Lang::Other(code.to_string()));
    }
    if is_year {
      // Languages with no year rule of their own leave this empty.
      if let Ok(year) = numbertext::year(code, &n.to_string()) {
        if !year.trim().is_empty() {
          return year.replace('-', " ");
        }
      }
    }
    return numbertext::cardinal(code, &n.to_string())
      .unwrap_or_default()
      .replace('-', " ");
  }
  num2words2_core::get_lang_by_key(code)
    .and_then(|l| l.to_cardinal(&num_bigint::BigInt::from(n)).ok())
    .unwrap_or_default()
}

fn digit_words(digits: &str, lang: &Lang) -> String {
  digits
    .chars()
    .filter_map(|c| c.to_digit(10))
    .map(|d| match lang {
      Lang::En => en_cardinal(d as u64),
      Lang::Es => es_cardinal(d as u64),
      Lang::Other(c) => other_words(c, d as u64, false),
    })
    .collect::<Vec<_>>()
    .join(if matches!(lang, Lang::Other(c) if c == "zh") {
      ""
    } else {
      " "
    })
}

const EN_ONES: [&str; 20] = [
  "zero",
  "one",
  "two",
  "three",
  "four",
  "five",
  "six",
  "seven",
  "eight",
  "nine",
  "ten",
  "eleven",
  "twelve",
  "thirteen",
  "fourteen",
  "fifteen",
  "sixteen",
  "seventeen",
  "eighteen",
  "nineteen",
];
const EN_TENS: [&str; 10] = [
  "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn en_below_100(n: u64) -> String {
  if n < 20 {
    EN_ONES[n as usize].to_string()
  } else if n % 10 == 0 {
    EN_TENS[(n / 10) as usize].to_string()
  } else {
    format!(
      "{} {}",
      EN_TENS[(n / 10) as usize],
      EN_ONES[(n % 10) as usize]
    )
  }
}

fn en_below_1000(n: u64) -> String {
  if n < 100 {
    en_below_100(n)
  } else if n % 100 == 0 {
    format!("{} hundred", EN_ONES[(n / 100) as usize])
  } else {
    format!(
      "{} hundred {}",
      EN_ONES[(n / 100) as usize],
      en_below_100(n % 100)
    )
  }
}

fn en_cardinal(n: u64) -> String {
  if n < 1000 {
    return en_below_1000(n);
  }
  let mut parts: Vec<String> = Vec::new();
  for (scale, name) in [
    (1_000_000_000, "billion"),
    (1_000_000, "million"),
    (1_000, "thousand"),
  ] {
    let group = (n / scale) % 1000;
    if group > 0 {
      parts.push(format!("{} {}", en_below_1000(group), name));
    }
  }
  if n % 1000 > 0 {
    parts.push(en_below_1000(n % 1000));
  }
  parts.join(" ")
}

/// 1950 -> "nineteen fifty", 1066 -> "ten sixty six", 2100 -> "twenty one hundred", 1905 -> "nineteen oh five", 2005 -> "two thousand five".
fn en_year(n: u64) -> String {
  // "one thousand", "two thousand five": no hundreds to split on.
  if n == 1000 || (2000..2010).contains(&n) {
    return en_cardinal(n);
  }
  let (hi, lo) = (n / 100, n % 100);
  match lo {
    0 => format!("{} hundred", en_below_100(hi)),
    1..=9 => format!("{} oh {}", en_below_100(hi), en_below_100(lo)),
    _ => format!("{} {}", en_below_100(hi), en_below_100(lo)),
  }
}

const ES_UNDER_30: [&str; 30] = [
  "cero",
  "uno",
  "dos",
  "tres",
  "cuatro",
  "cinco",
  "seis",
  "siete",
  "ocho",
  "nueve",
  "diez",
  "once",
  "doce",
  "trece",
  "catorce",
  "quince",
  "dieciséis",
  "diecisiete",
  "dieciocho",
  "diecinueve",
  "veinte",
  "veintiuno",
  "veintidós",
  "veintitrés",
  "veinticuatro",
  "veinticinco",
  "veintiséis",
  "veintisiete",
  "veintiocho",
  "veintinueve",
];
const ES_TENS: [&str; 10] = [
  "",
  "",
  "",
  "treinta",
  "cuarenta",
  "cincuenta",
  "sesenta",
  "setenta",
  "ochenta",
  "noventa",
];
const ES_HUNDREDS: [&str; 10] = [
  "",
  "ciento",
  "doscientos",
  "trescientos",
  "cuatrocientos",
  "quinientos",
  "seiscientos",
  "setecientos",
  "ochocientos",
  "novecientos",
];

fn es_below_100(n: u64) -> String {
  if n < 30 {
    ES_UNDER_30[n as usize].to_string()
  } else if n % 10 == 0 {
    ES_TENS[(n / 10) as usize].to_string()
  } else {
    format!(
      "{} y {}",
      ES_TENS[(n / 10) as usize],
      ES_UNDER_30[(n % 10) as usize]
    )
  }
}

fn es_below_1000(n: u64) -> String {
  if n == 100 {
    return "cien".to_string();
  }
  if n < 100 {
    return es_below_100(n);
  }
  let hundreds = ES_HUNDREDS[(n / 100) as usize];
  if n % 100 == 0 {
    hundreds.to_string()
  } else {
    format!("{} {}", hundreds, es_below_100(n % 100))
  }
}

/// A count in front of "mil" / "millones" drops the final "o" of "uno".
fn es_apocope(words: String) -> String {
  if let Some(stem) = words.strip_suffix("veintiuno") {
    format!("{stem}veintiún")
  } else if let Some(stem) = words.strip_suffix("uno") {
    format!("{stem}un")
  } else {
    words
  }
}

fn es_cardinal(n: u64) -> String {
  if n < 1000 {
    return es_below_1000(n);
  }
  let mut parts: Vec<String> = Vec::new();
  let millions = n / 1_000_000;
  if millions == 1 {
    parts.push("un millón".to_string());
  } else if millions > 1 {
    parts.push(format!("{} millones", es_apocope(es_cardinal(millions))));
  }
  let thousands = (n / 1000) % 1000;
  if thousands == 1 {
    parts.push("mil".to_string());
  } else if thousands > 1 {
    parts.push(format!("{} mil", es_apocope(es_below_1000(thousands))));
  }
  if n % 1000 > 0 {
    parts.push(es_below_1000(n % 1000));
  }
  parts.join(" ")
}

#[cfg(test)]
mod tests {
  use super::expand;

  #[test]
  fn english_years_are_read_as_years() {
    assert_eq!(expand("born in 1950.", "en"), "born in nineteen fifty.");
    assert_eq!(expand("in 1905", "en"), "in nineteen oh five");
    assert_eq!(expand("in 1900", "en"), "in nineteen hundred");
    assert_eq!(expand("in 2005", "en"), "in two thousand five");
    assert_eq!(expand("in 2000", "en"), "in two thousand");
    assert_eq!(expand("in 2026", "en"), "in twenty twenty six");
    assert_eq!(expand("in 2010", "en"), "in twenty ten");
  }

  #[test]
  fn english_cardinals() {
    assert_eq!(expand("I have 3 apples", "en"), "I have three apples");
    assert_eq!(expand("342", "en"), "three hundred forty two");
    assert_eq!(
      expand("1,234,567", "en"),
      "one million two hundred thirty four thousand five hundred sixty seven"
    );
    assert_eq!(expand("5000", "en"), "five thousand");
    assert_eq!(expand("3.14", "en"), "three point one four");
    assert_eq!(expand("007", "en"), "zero zero seven");
    assert_eq!(expand("09", "en"), "nine");
  }

  #[test]
  fn spanish_cardinals() {
    assert_eq!(expand("en 1950", "es"), "en mil novecientos cincuenta");
    assert_eq!(expand("2026", "es"), "dos mil veintiséis");
    assert_eq!(expand("100", "es"), "cien");
    assert_eq!(expand("101", "es"), "ciento uno");
    assert_eq!(expand("31", "es"), "treinta y uno");
    assert_eq!(expand("21000", "es"), "veintiún mil");
    assert_eq!(expand("1.000.000", "es"), "un millón");
    assert_eq!(expand("2.500.000", "es"), "dos millones quinientos mil");
    assert_eq!(expand("3,5", "es"), "tres coma cinco");
  }

  #[test]
  fn other_languages_use_their_own_words() {
    assert_eq!(expand("1950", "de"), "neunzehnhundertfünfzig");
    assert_eq!(expand("1950", "sv"), "nittonhundrafemtio");
    assert_eq!(expand("1950", "fr"), "mille neuf cent cinquante");
    assert_eq!(expand("1950", "zh"), "一九五零");
    assert_eq!(expand("3.14", "de"), "drei Komma eins vier");
    assert_eq!(expand("1.500", "de"), "eintausendfünfhundert");
    assert_eq!(expand("1950", "pt-BR"), "mil novecentos e cinquenta");
  }

  #[test]
  fn languages_only_num2words_knows() {
    for l in ["ar", "bn", "gu", "hi", "kn", "pa", "sw", "ta", "te"] {
      let said = expand("in 1950.", l);
      assert!(!said.chars().any(|c| c.is_ascii_digit()), "{l}: {said}");
    }
  }

  #[test]
  fn every_language_leaves_no_digit_behind() {
    for l in [
      "en", "es", "zh", "ja", "pt", "it", "hi", "fr", "ar", "bn", "ca", "cs", "de", "el", "fi",
      "gu", "hu", "kn", "ko", "mr", "nl", "pa", "ru", "sv", "sw", "ta", "te", "tr", "bg", "hr",
      "da", "et", "id", "lt", "lv", "pl", "ro", "sk", "sl", "uk", "vi",
    ] {
      for n in ["0", "7", "42", "1950", "2005", "123456", "1000000"] {
        let said = expand(n, l);
        assert!(
          !said.trim().is_empty() && !said.chars().any(|c| c.is_ascii_digit()),
          "{l} {n}: {said:?}"
        );
      }
    }
  }

  #[test]
  fn year_range_is_1000_to_2200() {
    assert_eq!(expand("1000", "en"), "one thousand");
    assert_eq!(expand("1066", "en"), "ten sixty six");
    assert_eq!(expand("1001", "en"), "ten oh one");
    assert_eq!(expand("2100", "en"), "twenty one hundred");
    assert_eq!(expand("2200", "en"), "twenty two hundred");
    assert_eq!(expand("999", "en"), "nine hundred ninety nine");
    assert_eq!(expand("2201", "en"), "two thousand two hundred one");
    assert_eq!(expand("1066", "de"), "tausendsechsundsechzig");
  }

  #[test]
  fn english_ordinals_and_decades() {
    assert_eq!(
      expand("the 1st, 2nd, 3rd and 4th", "en"),
      "the first, second, third and fourth"
    );
    assert_eq!(expand("21st", "en"), "twenty first");
    assert_eq!(expand("the 20th century", "en"), "the twentieth century");
    assert_eq!(expand("100th", "en"), "one hundredth");
    assert_eq!(expand("the 1950s", "en"), "the nineteen fifties");
    assert_eq!(expand("the 90s", "en"), "the nineties");
    assert_eq!(expand("mp3 and 5x", "en"), "mp3 and 5x");
  }

  #[test]
  fn separators_between_numbers_keep_them_apart() {
    assert_eq!(expand("1950-1960", "en"), "nineteen fifty nineteen sixty");
    assert_eq!(
      expand("12/05/1999", "en"),
      "twelve five nineteen ninety nine"
    );
    assert_eq!(expand("-5", "en"), "-five");
  }

  #[test]
  fn other_scripts_and_counters() {
    assert_eq!(expand("1950年", "zh"), "一九五零年");
    assert_eq!(expand("２０２４年", "ja").contains(char::is_numeric), false);
    for (l, n) in [
      ("hi", "१९५०"),
      ("ar", "١٩٥٠"),
      ("bn", "১৯৫০"),
      ("ta", "௧௯௫௦"),
    ] {
      let said = expand(n, l);
      assert!(
        !said.is_empty() && !said.chars().any(char::is_numeric),
        "{l}: {said}"
      );
    }
  }

  #[test]
  fn every_year_in_every_language() {
    for l in [
      "en", "es", "zh", "ja", "pt", "it", "hi", "fr", "ar", "bn", "ca", "cs", "de", "el", "fi",
      "gu", "hu", "kn", "ko", "mr", "nl", "pa", "ru", "sv", "sw", "ta", "te", "tr", "bg", "hr",
      "da", "et", "id", "lt", "lv", "pl", "ro", "sk", "sl", "uk", "vi",
    ] {
      for y in 990..2210 {
        let said = expand(&y.to_string(), l);
        assert!(
          !said.trim().is_empty() && !said.chars().any(|c| c.is_ascii_digit()),
          "{l} {y}: {said:?}"
        );
      }
    }
  }

  #[test]
  fn names_and_other_languages_are_untouched() {
    assert_eq!(
      expand("mp3 and H2O and 3D and 5x", "en"),
      "mp3 and H2O and 3D and 5x"
    );
    assert_eq!(expand("1950", "xx"), "1950");
  }
}
