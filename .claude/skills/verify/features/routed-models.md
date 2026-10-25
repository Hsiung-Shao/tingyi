# 專用模型下載與「專用模型」引擎

## Sub-features

- 「辨識引擎」下拉下方的小卡:日文 ReazonSpeech k2 v2(160 MB)、英文 Parakeet TDT 0.6B v2(661 MB)各一列,分開下載,可同時下載。
- 每列狀態:「下載」→ 進度條、百分比、速度與「取消」→ 取消或中斷後「繼續(x%)」→ 校驗中 → 「已下載」。
- 來源只有 Hugging Face 固定 commit(逐檔,Range 續傳,SHA256 校驗);暫存在 `models/.download/<dir>/`,全部校驗通過才放進正式目錄。
- 錯誤列(`data-role=model-error`)附替代方案:repo 的 `scripts/download-models.ps1 -WithReazonSpeech / -WithParakeet`。
- 「專用模型」引擎只有在 SenseVoice 存在、且日文或英文至少一個模型每個檔案大小都正確時才可選;下載完成會重查。
- 執行中下載完成:顯示「下載完成,下次按「開始」時生效。」。
- 專用模型建立失敗(壞檔等):該語言退回 SenseVoice,錯誤列出現「日文專用模型載入失敗,改用 SenseVoice:…」,狀態列與 log 的「模型載入完成:routed[…]」列出實際載入的模型。
- 授權列:兩個模型的授權與連結(用系統瀏覽器開啟)。

## How to get to it (user POV)

主視窗左欄「辨識引擎」下拉的正下方。還沒下載時,「專用模型(CPU…)」選項是灰的;按日文那列的「下載」,完成後就能選。

## Driving it with CDP

`scripts/verify_model_download.py`:
1. 架假 HF:`/<repo>/resolve/<commit>/<file>` 回 307 到 `/cdn/<repo>/<file>`,`/cdn/` 支援 Range/206、限速 20 MB/s,檔案取自 `D:\codeproject\translate\data\models`。
2. `TINGYI_MODEL_BASE_URL` 指向它、`TINGYI_DATA_DIR` 指向證據目錄下的臨時資料目錄(只用硬連結放 SenseVoice 與 VAD)。
3. 旅程:專用模型不可選 → 按日文「下載」→ 出現百分比與「取消」→ 取消 → 「繼續」且 `.part` > 0 → 按繼續 → 伺服器收到 `Range: bytes=K-`(K > 0)→ 「已下載」、正式目錄四個檔 → 專用模型可選 → 選專用模型、鎖 ja、開始 → 停止 → app.log 的「模型載入完成」那行含 `reazonspeech`。
4. 列的把手:`[data-model-id=reazonspeech]`、`[data-role=model-ready|model-progress|model-error]`。

## Gotchas

- 假伺服器不限速時日文模型一秒內就下完,來不及按「取消」;限速保留到能看到進度。
- 只驗了日文;英文 Parakeet(661 MB)的 app 內下載只跑過單元測試與相同程式路徑,真網路下載未在 GUI 驗。
- 真網路的 Hugging Face(307 到簽名 CDN)只由 `tingyi-bench download-model` 冒煙驗過,GUI 旅程用的是本機替身;替身不會有 CDN 網址過期、HF 限流(429)、代理伺服器等情況。
- 磁碟不足、防毒鎖檔只在單元測試以錯誤碼模擬。
