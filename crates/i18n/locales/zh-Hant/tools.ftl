# Imported from Zuse desktop catalogs. See migration.json for provenance.
# Edit individual translations here; do not regenerate this file.

tools-code_block_copy_code = 複製程式碼

tools-tool_row_error = 錯誤

tools-tool_row_no_matches = 沒有匹配項。

tools-tool_row_empty_directory = 空目錄。

tools-tool_row_plan = 計畫

tools-tool_row_no_plan_body = （沒有計畫正文。）

tools-tool_row_approved = 已核准

tools-tool_row_cancelled = 已取消

tools-tool_row_sending_to_thread = 正在傳送到執行緒…

tools-tool_row_creating_session_tab = 正在建立工作階段索引標籤…

tools-tool_row_creating_chat = 正在建立聊天…

tools-tool_row_open_chat = 開啟聊天

tools-tool_row_chat_not_loaded_yet = 聊天尚未載入

tools-tool_row_cancelled_2 = （已取消）

tools-tool_row_question = 問題

tools-tool_row_questions = 問題

tools-tool_row_copy_q_a = 複製問答

tools-tool_row_answered = 已回答

tools-tool_row_user_input = 使用者輸入

tools-tool_row_result = 結果

tools-tool_row_error_2 = 錯誤

tools-tool_row_thought_content_was_redacted_by_the_model = 思考內容已被模型隱去。

tools-tool_row_the_model_produced_a_thinking_block_the_sdk_forwarded_its_signed_recei = 模型產生了思考塊（SDK 已轉送其簽名回執），但底層文字在到達 Zuse 之前就被 Anthropic 的 agent SDK 過濾掉了。不繞過官方 SDK，我們無法展示實際的思考內容。

tools-tool_row_thinking = 思考

tools-code_block_code = { $name }{ " " }程式碼

tools-tool_row_open = 開啟{ " " }{ $shownPath }

tools-tool_row_background_task = 後臺工作

tools-tool_row_viewed_image = 已檢視影象

tools-tool_row_generating_image = 正在產生影象

tools-tool_row_generated_image = 已產生影象

tools-tool_row_copy_image = 複製影象

tools-tool_row_open_image = 開啟影象

tools-tool_row_image_generation_failed = 影象產生失敗

tools-tool_row_preview_of = { $value1 }{ " " }預覽

tools-tool_row_grep = Grep

tools-tool_row_glob = Glob

tools-tool_row_list = 清單

tools-tool_row_agent = 代理程式

tools-tool_row_webfetch = 網頁抓取

tools-tool_row_websearch = 網頁搜尋

tools-tool_row_todowrite = TodoWrite

tools-tool_row_spawn_agent = 啟動{ " " }{ $n }{ " " }個代理程式

tools-tool_row_browse = 瀏覽

tools-tool_row_screenshot = 截圖

tools-tool_row_read_page = 讀取頁面

tools-tool_row_click = 點選

tools-tool_row_type = 輸入

tools-tool_row_wait = 等待

tools-tool_row_scroll = 滾動

tools-tool_row_hover = 暫留

tools-tool_row_select = 選擇

tools-tool_row_press = 按鍵

tools-tool_row_console = 控制檯

tools-tool_row_log_in = 登入

tools-tool_row_max = 最長{ " " }{ $value1 }{ " " }·{ " " }

tools-tool_row_failed = 失敗

tools-tool_row_prompt = 提示

tools-tool_row_reply = 回覆

tools-tool_row_no_results_returned = （沒有返回結果）

tools-tool_row_threads = 討論串：

tools-tool_row_captured_the_visible_page = 已擷取可見頁面。

tools-tool_row_browser_screenshot = 瀏覽器截圖

tools-tool_row_pattern_sentence = 模式 <part0>{ $pattern }</part0>

tools-tool_row_scope_sentence = 範圍 <part0>{ $where }</part0>

tools-tool_row_model_sentence = 模型：<part0>{ $model }</part0>

tools-tool_row_live_states_tracked_sentence = 即時狀態：已跟蹤{ " " }{ $value }{ " " }項

tools-tool_row_spawn_agent_one = 啟動{ " " }{ $n }{ " " }個代理程式

tools-tool_row_spawn_agent_other = 啟動{ " " }{ $n }{ " " }個代理程式

tools-tool_row_edit_plural0_sentence = { $editCount }{ " " }處修改

tools-tool_row_edit_plural0_sentence_one = { $editCount }{ " " }處修改

tools-tool_row_edit_plural0_sentence_other = { $editCount }{ " " }處修改

tools-code_block_copy = 複製{ " " }{ $name }

tools-code_block_empty = （空）

tools-tool_row_empty = （空）

tools-tool_row_message_sent_to_thread = 訊息已傳送到執行緒

# Complete plural message. $count is numeric; remaining variables retain their source names.
tools-tool_row_spawn_agent-plural =
    { $count ->
       [one] 啟動{ " " }{ $n }{ " " }個代理程式
       *[other] 啟動{ " " }{ $n }{ " " }個代理程式
    }

# Complete plural message. $count is numeric; remaining variables retain their source names.
tools-tool_row_edit_plural0_sentence-plural =
    { $count ->
       [one] { $editCount }{ " " }處修改
       *[other] { $editCount }{ " " }處修改
    }
