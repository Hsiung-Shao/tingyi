# 計畫:翻譯模型對照(Sakura、Seed-X、LMT-60、TranslateGemma)

- 建立日期:2026-10-06
- 狀態:進行中
- 起因:使用者要「日文、英文各一個特化翻譯模型」,原則是免費、本機、能省效能更好。

## 候選與取捨

| 候選 | 類型 | 授權 | 決定 |
|---|---|---|---|
| SakuraLLM Sakura-7B-Qwen2.5-v1.0(IQ4_XS 4.25 GB) | 日→簡中特化(輕小說/Galgame 語料) | CC BY-NC-SA 4.0(禁商用) | **本輪對照** |
| SakuraLLM Sakura-1.5B-Qwen2.5-v1.0(F16 3.56 GB) | 同上,小模型 | 同上 | **本輪對照** |
| NiuTrans LMT-60(Qwen3 底,0.6B–8B) | 中英為核心的 60 語翻譯 | Apache-2.0 | 使用者未選,保留為英文特化候選 |
| Unbabel Tower-Plus-2B | 22 語翻譯(Gemma 2 底) | CC BY-NC-4.0 | 未選 |
| ByteDance Seed-X-PPO-7B | 28 語翻譯 | 開放權重 | 與 Hy-MT2 7B 同量級,省不到資源,未選 |

英→中目前沒有「只做英中」的成熟特化模型,LMT-60 是最接近的(中英為中心);等 Sakura 結果出來再決定要不要下載。

## 實作項目

- [x] `mt.rs`:`PromptStyle { HyMt2, Sakura }`,依模型檔名含 "sakura" 切換;Sakura 用 README v1.0 的 system / user 格式、術語表 `src->dst`、不送背景前文;取樣 temperature 0.1、top_p 0.3(`LlamaServerConfig::for_model`)。
- [x] `postprocess.rs`:OpenCC s2twp(`opencc-fmmseg`,MIT,純 Rust 字典內嵌)取代原本只靠內建詞表;Sakura 輸出簡體必須轉,Hy-MT2 繁體輸出照過一次當保險。
- [x] `paths.rs`:`KNOWN_MT_MODELS` 加兩個 Sakura 檔名,app 下拉選單可選。
- [x] `scripts/download-models.ps1`:`-WithSakura`。
- [x] app 與 bench 建翻譯器改走 `LlamaServerConfig::for_model`。
- [x] 30 句對照(`docs/bench/sentences-30.tsv`):Hy-MT2-7B-Q4_K_M vs Sakura-7B vs Sakura-1.5B,比首字 / 完成 / 顯示記憶體 / 日文品質(`bench-out/mt-sakura-*.jsonl`)。
- [x] 結果寫進 `docs/validation-report.md §6e`。
- [x] 使用者看過 Sakura 結果後要求再看其他模型 → 下載 Seed-X-PPO-7B、LMT-60 4B/8B、TranslateGemma 4B/12B。
- [x] `mt.rs` 加 SeedX / Lmt / TranslateGemma 範本與 `/completion` 原始端點;`llama.rs` 原始端點模型用 `--no-jinja`。
- [x] 五個候選 30 句對照,結果與結論寫進 `docs/validation-report.md §6f`。
- [ ] 使用者決定:維持 Hy-MT2 7B 預設?是否把 TranslateGemma-4B 列為省資源選項?
- [ ] `/handoff-check`。

## 已知限制

- Sakura 只會日文;英文句子仍需 Hy-MT2。若採用,需做「依來源語言選模型」,等於同時載兩個 llama-server(顯示記憶體 +4 GB 以上),這是取捨點。
- Sakura 語料是書面對話,直播口語效果要看實測。
- Sakura 禁商用;個人使用無虞,之後若要發佈需注意。
