# 遊戲術語包(流放之路)

從本機 POE DB(`pob-zh-engine/tools/ggpk_zh` 的版本化 SQLite)產生聽譯用的術語包。
術語包是 GGG 的遊戲字串,**只放在資料目錄 `data/terms/`,不進 repo、不打包進公開版**;
聽譯偵測到 `data/terms/*.json` 才在字幕頁「語言」卡顯示「遊戲術語」。

```
python scripts/poe-terms/build_term_pack.py --game poe1      # → data/terms/poe1.json
python scripts/poe-terms/build_term_pack.py --game poe2      # → data/terms/poe2.json
python -m pytest scripts/poe-terms/test_build_term_pack.py -q
```

換季(POE DB 建了新快照)後重跑一次即可;聽譯下次按「開始」就用新的。
術語包檔名(`--game`)只能是英數、底線、連字號;備份請改名成例如 `poe1-old.json`(`poe1.bak.json` 不會被列出)。
安裝版在沒有專案 `data/` 的電腦上,資料目錄是 `%LOCALAPPDATA%\tingyi`,這時用 `--out %LOCALAPPDATA%\tingyi\terms`。

## 選了術語包後聽譯做什麼
1. **誤聽修正**:原文裡 `aliases.tsv` 的誤聽/美式拼寫(不分大小寫、整字)改成正確英文,在使用者「辨識取代表」之後。
2. **Whisper 熱詞**:`hotwords-<game>.txt` 中術語包有的詞,當 Whisper 的 prompt(只放正確拼寫,≤ 400 字元)。SenseVoice 不支援。
3. **翻譯術語**:每句命中的術語(整字、含複數、長詞優先、最多 20 個)與使用者術語表合併交給翻譯模型;同一原文以使用者術語表為準。

## 篩選規則(`build_term_pack.py`)
- 只取官方有翻譯(`same_as_en=0`)、1–5 個詞、首字大寫的短專名;排除商城外觀、成就等表。
- 單字詞只收專名表(ascendancy、npcs、worldareas、characters、currencyitems、baseitemtypes)或 `allow-<game>.txt`;
  `stopwords.txt` 與「在資料庫英文句子裡出現過小寫」的一般英文字一律不收(避免 beach → 白沙灘頭)。
- 一個英文多個官方譯名取出現最多者,其他列在 `alt`。

## 手動調整
| 檔案 | 用途 |
|---|---|
| `overrides-<game>.tsv` | `英文<TAB>中文` 新增或改譯;`英文<TAB>` 刪除;中文填英文本身 = 譯文保留英文(例 `Path of Exile`) |
| `allow-<game>.txt` | 單字專名白名單 |
| `stopwords.txt` | 一律不收的常用字 |
| `aliases.tsv` | 誤聽 → 正確英文 |
| `hotwords-<game>.txt` | Whisper 熱詞,依優先序 |

`aliases.tsv`、`hotwords-*.txt` 的種子參考 [exilemoulei/shengmu-live-captions](https://github.com/exilemoulei/shengmu-live-captions)(MIT License)。
