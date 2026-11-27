use super::path::{parse_path, Segment};
use super::*;

const SAMPLE: &str = r#"{"menuId": 701011,"menuName": "预算和投入","menuNameEn": "","parentId": 7010,"icon": "","routePath": "bugetInvest","sort": 2.0,"visible": true,"subList": [],"todoCount": 0}"#;

#[test]
fn formats_like_the_utools_screenshot() {
    let expected = r#"{
    "menuId": 701011,
    "menuName": "预算和投入",
    "menuNameEn": "",
    "parentId": 7010,
    "icon": "",
    "routePath": "bugetInvest",
    "sort": 2.0,
    "visible": true,
    "subList": [],
    "todoCount": 0
}"#;
    assert_eq!(format(SAMPLE, Indent::Four).unwrap(), expected);
}

#[test]
fn keeps_number_literals_order_and_big_integers() {
    let src = r#"{"z":1.50,"a":12345678901234567890123,"e":-1E+5,"z":0}"#;
    assert_eq!(
        format(src, Indent::Two).unwrap(),
        "{\n  \"z\": 1.50,\n  \"a\": 12345678901234567890123,\n  \"e\": -1E+5,\n  \"z\": 0\n}"
    );
    assert_eq!(minify(&format(src, Indent::Tab).unwrap()).unwrap(), src);
}

#[test]
fn formats_nested_and_empty_containers() {
    let src = "[1,[2,3],{},{ },[ ],{\"a\":{\"b\":[true,null]}}]";
    let expected = "[\n\t1,\n\t[\n\t\t2,\n\t\t3\n\t],\n\t{},\n\t{},\n\t[],\n\t{\n\t\t\"a\": {\n\t\t\t\"b\": [\n\t\t\t\ttrue,\n\t\t\t\tnull\n\t\t\t]\n\t\t}\n\t}\n]";
    assert_eq!(format(src, Indent::Tab).unwrap(), expected);
    assert_eq!(format("  42 ", Indent::Two).unwrap(), "42");
}

#[test]
fn keeps_comments_when_formatting() {
    let src = "// 配置\n{\"a\":1, // 行尾注释\n/* 独占一行 */\n\"b\": /* 行内 */ [2]}";
    let expected = "// 配置\n{\n  \"a\": 1, // 行尾注释\n  /* 独占一行 */\n  \"b\": /* 行内 */ [\n    2\n  ]\n}";
    let formatted = format(src, Indent::Two).unwrap();
    assert_eq!(formatted, expected);
    // 再次格式化结果不变。
    assert_eq!(format(&formatted, Indent::Two).unwrap(), formatted);
    // 行注释后面紧跟逗号时，逗号必须换行，否则会被注释吞掉。
    let tricky = format("[1 // x\n, 2]", Indent::Two).unwrap();
    assert!(parse(&tricky).is_ok(), "{tricky}");
    assert_eq!(minify(&tricky).unwrap(), "[1,2]");
}

#[test]
fn strips_comments() {
    let src = "{\"a\": \"http://x\" /* c */, // d\n\"b\": 2}";
    assert_eq!(strip_comments(src), "{\"a\": \"http://x\" , \n\"b\": 2}");
    assert_eq!(minify(src).unwrap(), "{\"a\":\"http://x\",\"b\":2}");
}

#[test]
fn escapes_and_unescapes() {
    let escaped = escape("{\n  \"a\": \"引号\\\"\",\n  \"b\": [1, 2]\n}").unwrap();
    assert_eq!(escaped, r#"{\"a\":\"引号\\\"\",\"b\":[1,2]}"#);
    assert_eq!(
        unescape(&escaped).as_deref(),
        Some(r#"{"a":"引号\"","b":[1,2]}"#)
    );
    assert_eq!(unescape(r#""{\"a\":1}""#).as_deref(), Some(r#"{"a":1}"#));
    assert_eq!(unescape("{\"a\":1}"), None);
}

#[test]
fn prepares_payloads() {
    assert_eq!(prepare("[1]", Indent::Two).text, "[\n  1\n]");
    let escaped = prepare(r#"{\"a\":1}"#, Indent::Two);
    assert_eq!(escaped.text, "{\n  \"a\": 1\n}");
    assert_eq!(escaped.note, Some("已去除转义"));
    let quoted = prepare(r#""{\"a\":[]}""#, Indent::Two);
    assert_eq!(quoted.text, "{\n  \"a\": []\n}");
    assert_eq!(quoted.note, Some("已去除一层转义"));
    // 普通字符串、非法 JSON 保持原样。
    assert_eq!(prepare("\"hello\"", Indent::Two).text, "\"hello\"");
    assert_eq!(prepare("{a:1}", Indent::Two).text, "{a:1}");
}

#[test]
fn detects_json_like_selections() {
    assert!(looks_like_json(SAMPLE));
    assert!(looks_like_json("  [1, 2]\n"));
    assert!(looks_like_json("{oops}"), "与 uTools 相同，只看首尾括号");
    assert!(looks_like_json(r#""{\"a\":1}""#));
    assert!(!looks_like_json("hello"));
    assert!(!looks_like_json("{"));
    assert!(!looks_like_json("\"just a string\""));
}

fn error(src: &str) -> (usize, usize, String) {
    let err = parse(src).unwrap_err();
    (err.line, err.column, err.message)
}

#[test]
fn reports_error_positions() {
    assert_eq!(error("{\n  \"a\": 1\n  \"b\": 2\n}").0, 3);
    assert_eq!(error("{\n  \"a\": 1\n  \"b\": 2\n}").1, 3);
    assert!(error("[1,]").2.contains("多余的逗号"));
    assert!(error("{'a': 1}").2.contains("双引号"));
    assert!(error("{\"a\": undefined}").2.contains("undefined"));
    assert!(error("[01]").2.contains("无效的数字"));
    assert!(error("[1.]").2.contains("无效的数字"));
    assert!(error("{\"a\": \"x").2.contains("没有结束"));
    assert!(error("\"\\x\"").2.contains("无效的转义"));
    assert!(error("\"tab\there\"").2.contains("制表符"));
    assert!(error("[1] 2").2.contains("多余的内容"));
    assert!(error("[1 /* x ]").2.contains("注释没有结束"));
    assert!(error("   ").2.contains("为空"));
    // 列号按字符计：中文算一列。
    assert_eq!(error("{\"中文\" 1}"), (1, 7, "键后面应为“:”".to_string()));
    assert!(parse(&"[".repeat(600))
        .unwrap_err()
        .message
        .contains("嵌套"));
    // 网页里的不换行空格按空白处理。
    assert!(parse("{\u{a0}\"a\":\u{a0}1}").is_ok());
}

#[test]
fn tokens_cover_input_without_panicking() {
    for src in [
        "\"\\é\"",
        "\"abc\\",
        "\"多字节\\",
        "/*",
        "/* 未结束",
        "'x'",
        "\u{1F600}",
        "-",
        "\"\r\n",
    ] {
        let tokens = tokenize(src);
        let joined: String = tokens.iter().map(|t| t.text(src)).collect();
        assert_eq!(joined, src);
        let _ = parse(src);
        let _ = reformat(src, "  ");
    }
}

#[test]
fn filters_by_path() {
    let src = r#"{"data": {"list": [{"name": "a", "n": 1.0}, {"name": "b"}], "my key": 2}}"#;
    assert_eq!(filter(src, ".data.list[0].n", Indent::Two).unwrap(), "1.0");
    assert_eq!(
        filter(src, "data.list[-1].name", Indent::Two).unwrap(),
        "\"b\""
    );
    assert_eq!(
        filter(src, "this.data[\"my key\"]", Indent::Two).unwrap(),
        "2"
    );
    assert_eq!(
        filter(src, ".data.list[].name", Indent::Two).unwrap(),
        "[\n  \"a\",\n  \"b\"\n]"
    );
    assert_eq!(
        filter(src, ".data.list[*].n", Indent::Two).unwrap(),
        "[\n  1.0,\n  null\n]"
    );
    assert_eq!(filter(src, ".data.list.length", Indent::Two).unwrap(), "2");
    assert_eq!(
        filter(src, "", Indent::Two).unwrap(),
        format(src, Indent::Two).unwrap()
    );
    assert!(filter(src, ".data.missing", Indent::Two)
        .unwrap_err()
        .contains("missing"));
    assert!(filter(src, ".data.list[5]", Indent::Two)
        .unwrap_err()
        .contains("超出范围"));
    assert!(filter(src, ".data.list.name", Indent::Two).is_err());
    assert!(filter(src, ".data[", Indent::Two).is_err());
    assert_eq!(
        parse_path("[0][1]").unwrap(),
        vec![Segment::Index(0), Segment::Index(1)]
    );
    assert_eq!(
        parse_path(".名称.*").unwrap(),
        vec![Segment::Key("名称".into()), Segment::Each]
    );
}
