"""build_term_pack.py 的產包規則測試(只看規格,不依賴實作細節)。"""
import importlib.util
import json
import sqlite3
from pathlib import Path

import pytest

SCRIPT = Path(__file__).with_name("build_term_pack.py")


def _load_module():
    spec = importlib.util.spec_from_file_location("build_term_pack", SCRIPT)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


@pytest.fixture
def env(tmp_path):
    """建立空 DB 與設定/輸出目錄,回傳操作用的小工具。"""
    db = tmp_path / "poe.db"
    con = sqlite3.connect(db)
    con.execute("CREATE TABLE version(id INTEGER PRIMARY KEY, game_version TEXT, complete INTEGER)")
    con.execute(
        "CREATE TABLE cell(version_id INTEGER, tbl TEXT, row INTEGER, col TEXT, ident TEXT,"
        " is_array INTEGER DEFAULT 0, en TEXT, zh_tw TEXT, same_as_en INTEGER)"
    )
    con.commit()
    cfg = tmp_path / "config"
    cfg.mkdir()
    out = tmp_path / "out"
    out.mkdir()

    class Env:
        def version(self, vid, gv, complete=1):
            con.execute("INSERT INTO version VALUES (?,?,?)", (vid, gv, complete))
            con.commit()

        def cell(self, tbl, row, en, zh, same=0, vid=1, col="Name"):
            con.execute(
                "INSERT INTO cell(version_id,tbl,row,col,ident,en,zh_tw,same_as_en)"
                " VALUES (?,?,?,?,?,?,?,?)",
                (vid, tbl, row, col, f"{tbl}.{row}", en, zh, same),
            )
            con.commit()

        def config(self, name, text):
            (cfg / name).write_text(text, encoding="utf-8")

        def build(self, game="poe1"):
            con.close()
            return _load_module().main(
                ["--game", game, "--db", str(db), "--out", str(out), "--config-dir", str(cfg)]
            )

        def result(self, game="poe1"):
            return json.loads((out / f"{game}.json").read_text(encoding="utf-8"))

    return Env()


def _by_en(pack):
    return {e["en"]: e for e in pack["entries"]}


def test_G1_多譯名取最多者且記錄替代(env):
    """同一英文有多個譯名:取次數最多者,其餘列 alt,count 為選中者次數。"""
    env.version(1, "3.29.3")
    for row in (1, 2, 3):
        env.cell("baseitemtypes", row, "Chaos Orb", "混沌石")
    env.cell("baseitemtypes", 4, "Chaos Orb", "渾沌石")
    assert env.build() == 0
    e = _by_en(env.result())["Chaos Orb"]
    assert e["zh"] == "混沌石"
    assert "渾沌石" in e["alt"]
    assert e["count"] == 3
    assert e["src"] == "baseitemtypes.Name"


def test_G2_不合格的列不收(env):
    """same_as_en、含 {0}、小寫片語、含數字的英文都不進 entries。"""
    env.version(1, "3.29.3")
    env.cell("baseitemtypes", 1, "Chaos Orb", "混沌石")
    env.cell("baseitemtypes", 2, "Hello World", "Hello World", same=1)
    env.cell("baseitemtypes", 3, "Gain Power", "獲得{0}")
    env.cell("baseitemtypes", 4, "hit the button", "按下按鈕")
    env.cell("baseitemtypes", 5, "Act 2", "第二章")
    assert env.build() == 0
    ens = set(_by_en(env.result()))
    assert "Chaos Orb" in ens
    assert not ({"Hello World", "Gain Power", "hit the button", "Act 2"} & ens)


def test_G3_單字詞只收專名表或白名單且排除停用字(env):
    """單字:非專名表不收、停用字不收;白名單與專名表收。"""
    env.version(1, "3.29.3")
    env.cell("activeskills", 1, "Spark", "火花")
    env.cell("baseitemtypes", 2, "Fire", "火焰")
    env.cell("npctalk", 3, "Wraeclast", "瓦爾克拉斯")
    env.cell("npcs", 4, "Kitava", "奇塔弗")
    env.config("stopwords.txt", "Fire\n")
    env.config("allow-poe1.txt", "Wraeclast\n")
    assert env.build() == 0
    ens = set(_by_en(env.result()))
    assert "Spark" not in ens
    assert "Fire" not in ens
    assert "Wraeclast" in ens
    assert "Kitava" in ens


def test_G4_overrides可新增改譯刪除(env):
    """overrides:新增、改譯、以空中文刪除。"""
    env.version(1, "3.29.3")
    env.cell("baseitemtypes", 1, "Chaos Orb", "混沌石")
    env.cell("npcs", 2, "Kitava", "奇塔弗")
    env.config("overrides-poe1.tsv", "Divine Orb\t神聖石\nKitava\t奇塔瓦\nChaos Orb\t\n")
    assert env.build() == 0
    m = _by_en(env.result())
    assert m["Divine Orb"]["zh"] == "神聖石"
    assert m["Kitava"]["zh"] == "奇塔瓦"
    assert "Chaos Orb" not in m


def test_G5_只用最新的完整版本(env):
    """只讀 complete=1 的最大 id,不選未完成的新版本。"""
    env.version(1, "3.29.3", complete=1)
    env.version(2, "3.30.0", complete=0)
    env.cell("baseitemtypes", 1, "Chaos Orb", "混沌石", vid=1)
    env.cell("baseitemtypes", 2, "Phantom Orb", "幻影石", vid=2)
    assert env.build() == 0
    pack = env.result()
    assert pack["game_version"] == "3.29.3"
    assert pack["db_version"] == 1
    assert "Phantom Orb" not in _by_en(pack)


def test_G6_對話與介面字串只收白名單_口語片語不收(env):
    """npctalk/clientstrings 只收白名單;虛詞開頭且其餘都是一般字的片語不收;只差大小寫合併成一筆。"""
    env.version(1, "3.29.3")
    env.cell("npctalk", 1, "The Boss", "老闆", col="DialogueOption")
    env.cell("clientstrings", 2, "Not Supported", "非輔助", col="Text")
    env.cell("clientstrings", 3, "Path of Exile", "流亡黯道", col="Text")
    env.cell("baseitemtypes", 4, "The Old Man", "老人與海")
    env.cell("baseitemtypes", 5, "The Searing Exarch", "焚界者")
    env.cell("flavourtext", 6, "the old man said the boss is here", "老人說老闆在這", col="Text")
    env.cell("baseitemtypes", 7, "Herald of Ash", "灰燼之捷")
    env.cell("baseitemtypes", 8, "Herald of Ash", "灰燼之捷")
    env.cell("activeskills", 9, "Herald Of Ash", "灰燼使者")
    env.config("allow-poe1.txt", "Path of Exile\n")
    assert env.build() == 0
    ens = [e["en"] for e in env.result()["entries"]]
    assert "The Boss" not in ens
    assert "Not Supported" not in ens
    assert "Path of Exile" in ens
    assert "The Old Man" not in ens
    assert "The Searing Exarch" in ens
    assert [e.lower() for e in ens].count("herald of ash") == 1
    assert _by_en(env.result()).get("Herald of Ash", {}).get("zh") == "灰燼之捷"


def test_G7_覆寫不分大小寫(env):
    """overrides 的英文與條目只差大小寫時仍改譯或刪除,不新增重複條目。"""
    env.version(1, "3.29.3")
    env.cell("baseitemtypes", 1, "Herald Of Ash", "灰燼使者")
    env.cell("baseitemtypes", 2, "Big Game", "大遊戲")
    env.config("overrides-poe1.tsv", "herald of ash\t灰燼之捷\nbig game\t\n")
    assert env.build() == 0
    ents = env.result()["entries"]
    assert [e["en"].lower() for e in ents].count("herald of ash") == 1
    assert next(e for e in ents if e["en"].lower() == "herald of ash")["zh"] == "灰燼之捷"
    assert all(e["en"].lower() != "big game" for e in ents)
