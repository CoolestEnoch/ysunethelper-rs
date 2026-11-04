use regex::Regex;
use std::collections::HashMap;
fn attrs(tag: &str) -> HashMap<String, String> {
    Regex::new(r#"(?is)([\w-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
        .unwrap()
        .captures_iter(tag)
        .map(|m| {
            (
                m[1].to_ascii_lowercase(),
                html_escape::decode_html_entities(m.get(2).or_else(|| m.get(3)).unwrap().as_str())
                    .into_owned(),
            )
        })
        .collect()
}
pub fn fields(page: &str) -> HashMap<String, String> {
    let forms = Regex::new(r"(?is)<form\b[^>]*>.*?</form>").unwrap();
    let inputs = Regex::new(r"(?is)<input\b[^>]*>").unwrap();
    let mut scopes: Vec<_> = forms.find_iter(page).map(|m| m.as_str()).collect();
    if scopes.is_empty() {
        scopes.push(page);
    }
    let mut result = HashMap::new();
    for scope in scopes {
        let tags: Vec<_> = inputs.find_iter(scope).map(|m| attrs(m.as_str())).collect();
        if !tags.iter().any(|a| {
            a.get("name").is_some_and(|s| s == "cllt")
                && a.get("value").is_some_and(|s| s == "userNameLogin")
        }) {
            continue;
        }
        for tag in tags {
            if !tag
                .get("type")
                .is_some_and(|s| s.eq_ignore_ascii_case("hidden"))
            {
                continue;
            }
            if let Some(name) = tag
                .get("name")
                .filter(|s| !s.is_empty())
                .or_else(|| tag.get("id"))
            {
                result.insert(name.clone(), tag.get("value").cloned().unwrap_or_default());
            }
        }
    }
    result
}
pub fn error_message(page: &str) -> String {
    let strip = Regex::new(r"(?s)<[^>]*>").unwrap();
    for selector in [
        "showErrorTip",
        "form-errorTip",
        "help-block",
        "reauth_error_submit",
    ] {
        let re = Regex::new(&format!(r#"(?is)<[a-z0-9]+\b[^>]*(?:\bid\s*=\s*['"]{selector}['"]|\bclass\s*=\s*['"][^'"]*\b{selector}\b[^'"]*['"])[^>]*>(.*?)</[a-z0-9]+>"#)).unwrap();
        if let Some(m) = re.captures(page) {
            let text = html_escape::decode_html_entities(&strip.replace_all(&m[1], ""))
                .trim()
                .to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}
pub fn challenge(page: &str) -> bool {
    [
        "reAuthCheck",
        "Multifactor",
        "isMultifactor",
        "reAuthType",
        "二次认证",
    ]
    .iter()
    .any(|k| page.contains(k))
        || Regex::new(r#"needCaptcha\s*=\s*['"]true['"]"#)
            .unwrap()
            .is_match(page)
}
