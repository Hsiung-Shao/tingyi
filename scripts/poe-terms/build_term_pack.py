"""從本機 POE DB(ggpk_zh.sqlite / ggpk2_zh.sqlite)產生聽譯的遊戲術語包。

術語包只放在資料目錄(`data/terms/<game>.json`),不進 repo、不打包進公開版:內容是 GGG 的遊戲字串。
聽譯偵測到術語包才在字幕頁顯示「遊戲術語」。換季後重跑一次即可。

用法:
  python scripts/poe-terms/build_term_pack.py --game poe1
  python scripts/poe-terms/build_term_pack.py --game poe2 --out D:/somewhere/terms

篩選(詳見各常數):只取官方有翻譯的短專名(1–5 個詞、首字大寫);單字詞只收專名表或白名單,
排除 stopwords.txt 的常用字;一個英文多個官方譯名取出現最多者,其他列在 alt;
最後套 overrides-<game>.tsv(新增、改譯、刪除)。
"""
import argparse
import collections
import datetime
import json
import re
import sqlite3
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent

DEFAULT_DB = {
    "poe1": Path(r"d:\codeproject\Pob2\pob-zh-engine\tools\ggpk_zh\db\ggpk_zh.sqlite"),
    "poe2": Path(r"d:\codeproject\Pob2\pob-zh-engine\tools\ggpk2_zh\db\ggpk2_zh.sqlite"),
}
LABEL = {"poe1": "流放之路", "poe2": "流放之路 2"}

# 單字詞只有出自這些表才收(其餘表的單字多半是一般英文,例如技能名 Spark、詞綴片段)。
# words 表收的是稀有物品名片段(Doom、Storm…),單字一律不收,多字傳奇名照收。
PROPER_TABLES = {"ascendancy", "npcs", "worldareas", "characters", "words", "currencyitems", "baseitemtypes"}
SINGLE_WORD_TABLES = PROPER_TABLES - {"words"}
# 整張不收:商城外觀、成就、季票獎勵等名稱,直播不會講,只會增加誤命中(PoE1 的 mtxtypes 就有 7,700 筆)
EXCLUDE_TABLES = {"mtxtypes", "achievements", "achievementsetrewards", "battlepassrewards", "mysteryboxes",
                  "shoptag", "music", "hideouts", "awarddisplay", "tieredmicrotransactionsvisuals"}
# 對話選項與介面字串:大量首字大寫的一般片語(The Boss、Not Supported、I Have),只收白名單
UI_TABLES = {"npctalk", "clientstrings", "clientstrings2", "queststates", "uitalktext", "commands",
              "npctextaudio", "charactertextaudio"}
# 這些字開頭、其餘又都是一般英文字的多字詞是口語片語(The Old Man、My Father),不收
FUNCTION_START = {"the", "a", "an", "i", "my", "your", "our", "his", "her", "their", "its", "these", "those",
                  "this", "that", "not", "no", "go", "to", "you", "we", "it", "is", "are", "be", "do", "what",
                  "how", "why", "who", "where", "when", "yes", "let's", "i'm", "we're", "it's"}
# 同一譯名次數平手時的出處優先序(越前面越優先)
TABLE_PRIORITY = ["baseitemtypes", "currencyitems", "ascendancy", "npcs", "worldareas", "characters", "words", "activeskills", "passiveskills"]
LOWER_OK = {"of", "the", "and", "a", "an", "to", "in", "on", "for", "at"}
CJK = re.compile(r"[\u4e00-\u9fff]")
BAD_EN = re.compile(r"[0-9{}%\n\r\t<>\[\]|/\\:;\"]")
BAD_ZH = re.compile(r"[{%<]")
WORD = re.compile(r"^[A-Z][A-Za-z'\-]*$")
# 句子裡的全小寫字(前後不接字母),用來判斷某個字是不是一般英文
LOWER_TOKEN = re.compile(r"(?<![A-Za-z])[a-z]{3,}(?![A-Za-z])")


def proper_name(en: str) -> bool:
    """1–5 個詞、每個實詞首字大寫(允許小寫虛詞與 's、-)。"""
    if BAD_EN.search(en) or en != en.strip() or "  " in en:
        return False
    words = en.split(" ")
    if not 1 <= len(words) <= 5:
        return False
    for i, w in enumerate(words):
        if i > 0 and w in LOWER_OK:
            continue
        if not WORD.match(w):
            return False
    return True


def read_lines(path: Path) -> list:
    if not path.is_file():
        return []
    out = []
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        if line.strip() and not line.lstrip().startswith("#"):
            out.append(line.rstrip("\r\n"))
    return out


def latest_complete(con) -> tuple:
    cols = {r[1] for r in con.execute("pragma table_info(version)")}
    gv = "game_version" if "game_version" in cols else "NULL"
    row = con.execute(f"select id, {gv} from version where complete = 1 order by id desc limit 1").fetchone()
    if not row:
        raise SystemExit("資料庫沒有完整的版本快照(version.complete=1)")
    return row[0], row[1] or ""


def build(game: str, db: Path, config: Path) -> dict:
    con = sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True)
    vid, game_version = latest_complete(con)
    allow = {w.strip().lower() for w in read_lines(config / f"allow-{game}.txt")}
    stop = {w.strip().lower() for w in read_lines(config / "stopwords.txt")}

    # 一般英文字:在資料庫英文句子(對話、說明)裡出現過小寫的字。真專名(Kitava)在句中一律大寫,
    # 地圖名這類一般字(Beach、Park、Vanity)會以小寫出現 → 單字詞排除,免得口語被套成遊戲譯名。
    common = set()
    for (text,) in con.execute("select en from cell where version_id = ? and instr(en, ' ') > 0", (vid,)):
        common.update(LOWER_TOKEN.findall(text or ""))

    # en → zh → Counter(tbl.col)
    seen: dict = collections.defaultdict(lambda: collections.defaultdict(collections.Counter))
    q = "select tbl, col, en, zh_tw from cell where version_id = ? and same_as_en = 0 and zh_tw is not null"
    for tbl, col, en, zh in con.execute(q, (vid,)):
        if tbl in EXCLUDE_TABLES:
            continue
        en = (en or "").strip()
        zh = (zh or "").strip()
        if not en or not zh or not CJK.search(zh) or BAD_ZH.search(zh) or not proper_name(en):
            continue
        low = en.lower()
        if tbl in UI_TABLES and low not in allow:
            continue
        if " " not in en:
            if low in stop:
                continue
            if low not in allow and (tbl not in SINGLE_WORD_TABLES or low in common):
                continue
        elif low not in allow:
            words = low.split(" ")
            if words[0] in FUNCTION_START and all(w in common or w in LOWER_OK for w in words[1:]):
                continue
        seen[en][zh][f"{tbl}.{col}"] += 1
    con.close()

    def rank(item):
        zh, srcs = item
        best_tbl = min((TABLE_PRIORITY.index(s.split(".")[0]) if s.split(".")[0] in TABLE_PRIORITY else 99) for s in srcs)
        return (-sum(srcs.values()), best_tbl, zh)

    # 只差大小寫的寫法(Herald of Ash / Herald Of Ash)合併:取出現最多的寫法當 en
    merged: dict = {}
    for en, by_zh in seen.items():
        key = en.lower()
        total = sum(sum(c.values()) for c in by_zh.values())
        if key not in merged:
            merged[key] = [en, total, collections.defaultdict(collections.Counter)]
        elif total > merged[key][1]:
            merged[key][0], merged[key][1] = en, total
        for zh, srcs in by_zh.items():
            merged[key][2][zh].update(srcs)

    entries = {}
    for en, _, by_zh in merged.values():
        ranked = sorted(by_zh.items(), key=rank)
        zh, srcs = ranked[0]
        entries[en] = {
            "en": en,
            "zh": zh,
            "src": srcs.most_common(1)[0][0],
            "count": sum(srcs.values()),
            "alt": [z for z, _ in ranked[1:]],
        }

    # 手動覆寫:英文<TAB>中文 新增或改譯;英文<TAB>(空)刪除。不分大小寫對照(大小寫已合併)
    by_lower = {k.lower(): k for k in entries}
    for line in read_lines(config / f"overrides-{game}.tsv"):
        en, _, zh = line.partition("\t")
        en, zh = en.strip(), zh.strip()
        if not en:
            continue
        key = by_lower.get(en.lower())
        if not zh:
            if key:
                entries.pop(key)
                by_lower.pop(en.lower())
        elif key:
            entries[key].update(zh=zh, src="override")
        else:
            entries[en] = {"en": en, "zh": zh, "src": "override", "count": 0, "alt": []}
            by_lower[en.lower()] = en

    known = {e.lower() for e in entries}
    aliases = []
    for line in read_lines(config / "aliases.tsv"):
        frm, _, to = line.partition("\t")
        if frm.strip() and to.strip().lower() in known:
            aliases.append({"from": frm.strip(), "to": to.strip()})
    hotwords = [w.strip() for w in read_lines(config / f"hotwords-{game}.txt") if w.strip().lower() in known]

    return {
        "game": game,
        "label": LABEL.get(game, game),
        "game_version": game_version,
        "db_version": vid,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "entries": sorted(entries.values(), key=lambda e: e["en"].lower()),
        "aliases": aliases,
        "hotwords": hotwords,
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="從本機 POE DB 產生聽譯遊戲術語包")
    ap.add_argument("--game", required=True, choices=["poe1", "poe2"])
    ap.add_argument("--db", help="ggpk_zh.sqlite 路徑(預設依 --game)")
    ap.add_argument("--out", default=str(ROOT / "data" / "terms"))
    ap.add_argument("--config-dir", default=str(HERE))
    a = ap.parse_args(argv)
    db = Path(a.db) if a.db else DEFAULT_DB[a.game]
    if not db.is_file():
        print(f"找不到資料庫:{db}", file=sys.stderr)
        return 1
    pack = build(a.game, db, Path(a.config_dir))
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"{a.game}.json"
    path.write_text(json.dumps(pack, ensure_ascii=False, indent=1), encoding="utf-8")
    singles = sum(1 for e in pack["entries"] if " " not in e["en"])
    print(f"{path}:{len(pack['entries'])} 詞(單字 {singles})、誤聽 {len(pack['aliases'])}、熱詞 {len(pack['hotwords'])};"
          f"遊戲版本 {pack['game_version']}、DB 版本 {pack['db_version']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
