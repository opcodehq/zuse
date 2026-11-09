# Imported from Zuse desktop catalogs. See migration.json for provenance.
# Edit individual translations here; do not regenerate this file.

tools-code_block_copy_code = 复制代码

tools-tool_row_error = 错误

tools-tool_row_no_matches = 没有匹配项。

tools-tool_row_empty_directory = 空目录。

tools-tool_row_plan = 计划

tools-tool_row_no_plan_body = （没有计划正文。）

tools-tool_row_approved = 已批准

tools-tool_row_cancelled = 已取消

tools-tool_row_sending_to_thread = 正在发送到线程…

tools-tool_row_creating_session_tab = 正在创建会话选项卡…

tools-tool_row_creating_chat = 正在创建聊天…

tools-tool_row_open_chat = 打开聊天

tools-tool_row_chat_not_loaded_yet = 聊天尚未加载

tools-tool_row_cancelled_2 = （已取消）

tools-tool_row_question = 问题

tools-tool_row_questions = 问题

tools-tool_row_copy_q_a = 复制问答

tools-tool_row_answered = 已回答

tools-tool_row_user_input = 用户输入

tools-tool_row_result = 结果

tools-tool_row_error_2 = 错误

tools-tool_row_thought_content_was_redacted_by_the_model = 思考内容已被模型隐去。

tools-tool_row_the_model_produced_a_thinking_block_the_sdk_forwarded_its_signed_recei = 模型生成了思考块（SDK 已转发其签名回执），但底层文本在到达 Zuse 之前就被 Anthropic 的 agent SDK 过滤掉了。不绕过官方 SDK，我们无法展示实际的思考内容。

tools-tool_row_thinking = 思考

tools-code_block_code = { $name }{ " " }代码

tools-tool_row_open = 打开{ " " }{ $shownPath }

tools-tool_row_background_task = 后台任务

tools-tool_row_viewed_image = 已查看图像

tools-tool_row_generating_image = 正在生成图像

tools-tool_row_generated_image = 已生成图像

tools-tool_row_copy_image = 复制图像

tools-tool_row_open_image = 打开图像

tools-tool_row_image_generation_failed = 图像生成失败

tools-tool_row_preview_of = { $value1 }{ " " }预览

tools-tool_row_grep = Grep

tools-tool_row_glob = Glob

tools-tool_row_list = 列表

tools-tool_row_agent = 智能体

tools-tool_row_webfetch = 网页抓取

tools-tool_row_websearch = 网页搜索

tools-tool_row_todowrite = TodoWrite

tools-tool_row_spawn_agent = 启动{ " " }{ $n }{ " " }个智能体

tools-tool_row_browse = 浏览

tools-tool_row_screenshot = 截图

tools-tool_row_read_page = 读取页面

tools-tool_row_click = 点击

tools-tool_row_type = 输入

tools-tool_row_wait = 等待

tools-tool_row_scroll = 滚动

tools-tool_row_hover = 悬停

tools-tool_row_select = 选择

tools-tool_row_press = 按键

tools-tool_row_console = 控制台

tools-tool_row_log_in = 登录

tools-tool_row_max = 最长{ " " }{ $value1 }{ " " }·{ " " }

tools-tool_row_failed = 失败

tools-tool_row_prompt = 提示

tools-tool_row_reply = 回复

tools-tool_row_no_results_returned = （没有返回结果）

tools-tool_row_threads = 线程：

tools-tool_row_captured_the_visible_page = 已截取可见页面。

tools-tool_row_browser_screenshot = 浏览器截图

tools-tool_row_pattern_sentence = 模式 <part0>{ $pattern }</part0>

tools-tool_row_scope_sentence = 范围 <part0>{ $where }</part0>

tools-tool_row_model_sentence = 模型：<part0>{ $model }</part0>

tools-tool_row_live_states_tracked_sentence = 实时状态：已跟踪{ " " }{ $value }{ " " }项

tools-tool_row_spawn_agent_one = 启动{ " " }{ $n }{ " " }个智能体

tools-tool_row_spawn_agent_other = 启动{ " " }{ $n }{ " " }个智能体

tools-tool_row_edit_plural0_sentence = { $editCount }{ " " }处修改

tools-tool_row_edit_plural0_sentence_one = { $editCount }{ " " }处修改

tools-tool_row_edit_plural0_sentence_other = { $editCount }{ " " }处修改

tools-code_block_copy = 复制{ " " }{ $name }

tools-code_block_empty = （空）

tools-tool_row_empty = （空）

tools-tool_row_message_sent_to_thread = 消息已发送到线程

# Complete plural message. $count is numeric; remaining variables retain their source names.
tools-tool_row_spawn_agent-plural =
    { $count ->
       [one] 启动{ " " }{ $n }{ " " }个智能体
       *[other] 启动{ " " }{ $n }{ " " }个智能体
    }

# Complete plural message. $count is numeric; remaining variables retain their source names.
tools-tool_row_edit_plural0_sentence-plural =
    { $count ->
       [one] { $editCount }{ " " }处修改
       *[other] { $editCount }{ " " }处修改
    }
