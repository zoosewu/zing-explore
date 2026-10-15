-- Lua 除錯器：DAP（Debug Adapter Protocol）的除錯邏輯。傳輸層在 Rust（luadebug.rs）：
--   __dbg_send(json)  送出訊息
--   __dbg_wait()      暫停時等下一個訊息；nil 表示連線中斷
--   __dbg_poll()      執行中查看有沒有訊息（不等待）
-- 載入後交給 prelude 的鉤子（__attach_debugger），閒置時收到的訊息由 Rust 呼叫 __debugger.handle(訊息)。
-- 鉤子執行期間 Lua 不會再觸發鉤子，所以除錯器本身的程式碼不會被中斷點或單步攔下。

local send, wait, poll = __dbg_send, __dbg_wait, __dbg_poll
local SCRIPTS = (__scripts_dir or ""):gsub("[\\/]+$", "")
local PRELUDE_SOURCE = __prelude_source or ""
local getinfo, getlocal, getupvalue = debug.getinfo, debug.getlocal, debug.getupvalue
local set_line_hook = __set_line_hook

-- ---------------------------------------------------------------- JSON

local ARRAY = {}
local function array(t) return setmetatable(t or {}, ARRAY) end

local ESC = { ['"'] = '\\"', ["\\"] = "\\\\", ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" }

local function encode(v)
  local t = type(v)
  if t == "string" then
    if not utf8.len(v) then v = v:gsub("[\128-\255]", "?") end
    return '"' .. v:gsub('[%c"\\]', function(c) return ESC[c] or string.format("\\u%04x", c:byte()) end) .. '"'
  elseif t == "number" then
    if v ~= v or v == math.huge or v == -math.huge then return "null" end
    return math.type(v) == "integer" and tostring(v) or string.format("%.17g", v)
  elseif t == "boolean" then
    return tostring(v)
  elseif t == "table" then
    local parts = {}
    if getmetatable(v) == ARRAY or #v > 0 then
      for i = 1, #v do parts[i] = encode(v[i]) end
      return "[" .. table.concat(parts, ",") .. "]"
    end
    for k, x in pairs(v) do parts[#parts + 1] = encode(tostring(k)) .. ":" .. encode(x) end
    return "{" .. table.concat(parts, ",") .. "}"
  end
  return "null"
end

local UNESC = { b = "\b", f = "\f", n = "\n", r = "\r", t = "\t", ['"'] = '"', ["\\"] = "\\", ["/"] = "/" }

local function decode(s)
  local pos = 1
  local function ws() pos = s:find("[^ \t\r\n]", pos) or #s + 1 end
  local function str()
    local out, i = {}, pos + 1
    while true do
      local c = s:sub(i, i)
      if c == '"' then
        pos = i + 1
        return table.concat(out)
      elseif c == "\\" then
        local e = s:sub(i + 1, i + 1)
        if e == "u" then
          local cp = tonumber(s:sub(i + 2, i + 5), 16) or 63
          i = i + 6
          if cp >= 0xD800 and cp <= 0xDBFF and s:sub(i, i + 1) == "\\u" then
            local lo = tonumber(s:sub(i + 2, i + 5), 16) or 0xDC00
            cp = 0x10000 + (cp - 0xD800) * 0x400 + (lo - 0xDC00)
            i = i + 6
          end
          out[#out + 1] = utf8.char(cp)
        else
          out[#out + 1] = UNESC[e] or e
          i = i + 2
        end
      elseif c == "" then
        error("JSON 字串沒有結束")
      else
        local j = s:find('["\\]', i) or #s + 1
        out[#out + 1] = s:sub(i, j - 1)
        i = j
      end
    end
  end
  local value
  local function list(close, item)
    pos = pos + 1
    ws()
    if s:sub(pos, pos) == close then pos = pos + 1 return end
    while true do
      item()
      ws()
      local c = s:sub(pos, pos)
      pos = pos + 1
      if c == close then return end
      if c ~= "," then error("JSON 格式錯誤（位置 " .. pos .. "）") end
    end
  end
  function value()
    ws()
    local c = s:sub(pos, pos)
    if c == "{" then
      local t = {}
      list("}", function()
        ws()
        local k = str()
        ws()
        pos = pos + 1 -- :
        t[k] = value()
      end)
      return t
    elseif c == "[" then
      local t = array()
      list("]", function() t[#t + 1] = value() end)
      return t
    elseif c == '"' then
      return str()
    end
    for word, v in pairs({ ["true"] = true, ["false"] = false, ["null"] = ARRAY }) do
      if s:sub(pos, pos + #word - 1) == word then
        pos = pos + #word
        if v == ARRAY then return nil end
        return v
      end
    end
    local num = s:match("^-?[%d.eE+-]+", pos)
    if not num then error("JSON 格式錯誤（位置 " .. pos .. "）") end
    pos = pos + #num
    return math.tointeger(tonumber(num)) or tonumber(num)
  end
  return value()
end

-- ---------------------------------------------------------------- 狀態

local M = { encode = encode, decode = decode }
local seq = 0
local attached = false
local bps = {}            -- 正規化路徑 → { [行] = 條件字串或 true }
local bp_lines = {}       -- 任何檔案有中斷點的行號（快速過濾）
local break_on_error = true
local step = nil          -- { kind = "in" | "over" | "out", depth = 數字 }
local pause_req = false
local busy = false        -- 處理閒置訊息時，不讓行鉤子攔下除錯器以外的程式碼
local stopped = nil       -- 暫停中：{ base_bottom = { [框架] = 距離底部 }, depth = 數字 }
local refs, nextref = {}, 1

local function emit(msg)
  seq = seq + 1
  msg.seq = seq
  send(encode(msg))
end

local function event(name, body) emit({ type = "event", event = name, body = body }) end

local function respond(req, body, err)
  emit({
    type = "response", request_seq = req.seq, command = req.command,
    success = err == nil, message = err, body = body,
  })
end

local function update_hook()
  set_line_hook(attached and (next(bps) ~= nil or step ~= nil or pause_req))
end

-- 腳本的 chunk 名稱是「@檔名」（相對於 scripts 資料夾），中斷點用完整路徑：兩邊都轉成小寫的完整路徑比對
local function norm(p)
  p = p:gsub("/", "\\")
  if not p:match("^%a:\\") and not p:match("^\\\\") then p = SCRIPTS .. "\\" .. p end
  return p:lower()
end

local function source_of(info)
  local src = info.source or ""
  if src == "@prelude.lua" then
    return { name = "prelude.lua", sourceReference = 1, presentationHint = "deemphasize" }
  elseif src:sub(1, 1) == "@" then
    local name = src:sub(2)
    return { name = name:match("[^\\/]+$") or name, path = SCRIPTS .. "\\" .. name }
  end
  return nil
end

-- 從 level（相對於呼叫 count_from 的函式往上算，1 為 count_from 本身）到堆疊底部的框架數
local function count_from(level)
  local n = 0
  while getinfo(level + n, "l") do n = n + 1 end
  return n
end

-- 暫停時第 k 個框架（0 為最上層）相對於呼叫者的層級
local function level_of(k)
  local from_bottom = stopped and stopped.from_bottom[k]
  if not from_bottom then return nil end
  return count_from(3) - from_bottom
end

-- ---------------------------------------------------------------- 變數

local function ref(entry)
  refs[nextref] = entry
  nextref = nextref + 1
  return nextref - 1
end

local function show(v)
  local t = type(v)
  if t == "string" then
    local s = #v > 300 and v:sub(1, 300) .. "…" or v
    return '"' .. s:gsub("[\r\n\t]", { ["\r"] = "\\r", ["\n"] = "\\n", ["\t"] = "\\t" }) .. '"', "string", 0
  elseif t == "table" then
    local n = 0
    for _ in next, v do n = n + 1 if n > 999 then break end end
    local mt = getmetatable(v)
    local label = #v > 0 and ("{…} #" .. #v) or (n > 0 and "{…}" or (mt and "{…}" or "{}"))
    return label, "table", (n > 0 or mt) and ref({ kind = "table", value = v }) or 0
  elseif t == "function" then
    local i = getinfo(v, "S")
    return "function " .. (i.short_src or "?") .. ":" .. (i.linedefined or 0), "function", 0
  end
  return tostring(v), t, 0
end

local function variable(name, v)
  local value, t, r = show(v)
  return { name = tostring(name), value = value, type = t, variablesReference = r }
end

local function key_name(k)
  if type(k) == "string" and k:match("^[%a_][%w_]*$") then return k end
  if type(k) == "string" then return string.format("[%q]", k) end
  return "[" .. tostring(k) .. "]"
end

local function table_vars(t)
  local out = array()
  local entries = {}
  local ok = pcall(function()
    for k, v in pairs(t) do
      entries[#entries + 1] = { k, v }
      if #entries >= 1000 then break end
    end
  end)
  if not ok then
    for k, v in next, t do entries[#entries + 1] = { k, v } end
  end
  table.sort(entries, function(a, b)
    local ta, tb = type(a[1]), type(b[1])
    if ta == "number" and tb == "number" then return a[1] < b[1] end
    if ta ~= tb then return ta == "number" end
    return tostring(a[1]) < tostring(b[1])
  end)
  for _, e in ipairs(entries) do out[#out + 1] = variable(key_name(e[1]), e[2]) end
  local mt = getmetatable(t)
  if type(mt) == "table" then out[#out + 1] = variable("(metatable)", mt) end
  return out
end

-- 框架的區域變數與上值（名稱 → 值，同名時內層優先）
local function frame_vars(k)
  local L = level_of(k)
  local locals, ups = {}, {}
  if not L then return locals, ups end
  local i = 1
  while true do
    local name, v = getlocal(L, i)
    if not name then break end
    if name:sub(1, 1) ~= "(" then locals[#locals + 1] = { name, v } end
    i = i + 1
  end
  i = 1
  while true do
    local name, v = getlocal(L, -i)
    if not name then break end
    locals[#locals + 1] = { "...[" .. i .. "]", v }
    i = i + 1
  end
  local f = getinfo(L, "f").func
  i = 1
  while f do
    local name, v = getupvalue(f, i)
    if not name then break end
    if name ~= "_ENV" then ups[#ups + 1] = { name, v } end
    i = i + 1
  end
  return locals, ups
end

-- ---------------------------------------------------------------- 求值

local function evaluate(expr, k)
  local env = _G
  if k and stopped then
    local locals, ups = frame_vars(k)
    local scope = {}
    for _, e in ipairs(ups) do scope[e[1]] = e[2] end
    for _, e in ipairs(locals) do scope[e[1]] = e[2] end
    env = setmetatable({}, {
      __index = function(_, name)
        local v = scope[name]
        if v ~= nil then return v end
        return _G[name]
      end,
      __newindex = _G,
    })
  end
  local f, err = load("return " .. expr, "=求值", "t", env)
  if not f then f, err = load(expr, "=求值", "t", env) end
  if not f then return nil, err end
  local r = table.pack(pcall(f))
  if not r[1] then return nil, tostring(r[2]) end
  return r[2], nil, r.n > 1
end

-- ---------------------------------------------------------------- 請求

local handlers = {}

function handlers.initialize(req)
  respond(req, {
    supportsConfigurationDoneRequest = true,
    supportsEvaluateForHovers = true,
    supportsConditionalBreakpoints = true,
    supportsLogPoints = true,
    exceptionBreakpointFilters = array({ { filter = "error", label = "腳本錯誤時暫停", default = true } }),
  })
  event("initialized", {})
end

function handlers.attach(req)
  attached = true
  respond(req, {})
  event("output", { category = "console", output = "已連線到 zing（腳本資料夾：" .. SCRIPTS .. "）\n" })
end

function handlers.launch(req)
  respond(req, nil, "zing 的腳本除錯只支援附加（attach）：請先開啟 zing，再以 request = \"attach\" 連線")
end

function handlers.setBreakpoints(req)
  local args = req.arguments or {}
  local path = args.source and args.source.path
  local out = array()
  if path then
    local key = norm(path)
    local inside = key:sub(1, #SCRIPTS + 1) == (SCRIPTS .. "\\"):lower()
    local set = {}
    for _, b in ipairs(args.breakpoints or {}) do
      set[b.line] = { cond = b.condition, log = b.logMessage }
      out[#out + 1] = {
        verified = inside, line = b.line,
        message = not inside and "只能在 zing 的 scripts 資料夾裡的腳本設定中斷點" or nil,
      }
    end
    bps[key] = next(set) and set or nil
  end
  bp_lines = {}
  for _, set in pairs(bps) do
    for line in pairs(set) do bp_lines[line] = true end
  end
  update_hook()
  respond(req, { breakpoints = out })
end

function handlers.setExceptionBreakpoints(req)
  break_on_error = false
  for _, f in ipairs((req.arguments or {}).filters or {}) do
    if f == "error" then break_on_error = true end
  end
  respond(req, {})
end

function handlers.configurationDone(req) respond(req, {}) end

function handlers.threads(req)
  respond(req, { threads = array({ { id = 1, name = "Lua 腳本" } }) })
end

-- 經 xpcall 等方式呼叫時 Lua 推不出函式名稱：找全域變數裡的同一個函式
local function global_name(f)
  for k, v in next, _G do
    if rawequal(v, f) and type(k) == "string" then return k end
  end
  return nil
end

function handlers.stackTrace(req)
  local frames = array()
  if stopped then
    local args = req.arguments or {}
    local first = args.startFrame or 0
    local count = args.levels and args.levels > 0 and args.levels or 200
    for k = first, first + count - 1 do
      local L = level_of(k)
      local info = L and getinfo(L, "nSlf")
      if not info then break end
      local name = info.name or global_name(info.func)
        or (info.what == "main" and "（主程式）") or (info.what == "C" and "[C]") or "（匿名函式）"
      frames[#frames + 1] = {
        id = k + 1, name = name, source = source_of(info),
        line = info.currentline > 0 and info.currentline or 0, column = 1,
        presentationHint = info.what == "C" and "subtle" or nil,
      }
    end
    respond(req, { stackFrames = frames, totalFrames = #stopped.from_bottom + 1 })
  else
    respond(req, { stackFrames = frames, totalFrames = 0 })
  end
end

function handlers.scopes(req)
  local k = ((req.arguments or {}).frameId or 1) - 1
  respond(req, {
    scopes = array({
      { name = "區域變數", variablesReference = ref({ kind = "locals", frame = k }), expensive = false },
      { name = "上值", variablesReference = ref({ kind = "upvalues", frame = k }), expensive = false },
      { name = "全域", variablesReference = ref({ kind = "table", value = _G }), expensive = true },
    }),
  })
end

function handlers.variables(req)
  local entry = refs[(req.arguments or {}).variablesReference or 0]
  local out = array()
  if entry and entry.kind == "table" then
    out = table_vars(entry.value)
  elseif entry then
    local locals, ups = frame_vars(entry.frame)
    for _, e in ipairs(entry.kind == "locals" and locals or ups) do out[#out + 1] = variable(e[1], e[2]) end
  end
  respond(req, { variables = out })
end

function handlers.evaluate(req)
  local args = req.arguments or {}
  local k = args.frameId and args.frameId - 1
  local v, err, has = evaluate(args.expression or "", k)
  if err then return respond(req, nil, err) end
  if not has and args.context == "repl" then return respond(req, { result = "（沒有回傳值）", variablesReference = 0 }) end
  local value, t, r = show(v)
  respond(req, { result = value, type = t, variablesReference = r })
end

function handlers.source(req)
  local r = (req.arguments or {}).sourceReference
  if r == 1 then return respond(req, { content = PRELUDE_SOURCE }) end
  respond(req, nil, "沒有這個來源")
end

function handlers.pause(req)
  pause_req = true
  update_hook()
  respond(req, {})
end

local function resume(req, kind)
  if kind and stopped then
    step = { kind = kind, depth = stopped.depth }
  end
  respond(req, { allThreadsContinued = true })
  return "resume"
end

function handlers.continue(req) return resume(req, nil) end
handlers["next"] = function(req) return resume(req, "over") end
function handlers.stepIn(req) return resume(req, "in") end
function handlers.stepOut(req) return resume(req, "out") end

local function reset()
  attached, bps, bp_lines, step, pause_req = false, {}, {}, nil, false
  update_hook()
end

function handlers.disconnect(req)
  reset()
  respond(req, {})
  return "resume"
end
handlers.terminate = handlers.disconnect

local function dispatch(text)
  local ok, req = pcall(decode, text)
  if not ok or type(req) ~= "table" or req.type ~= "request" then return nil end
  local h = handlers[req.command]
  if not h then
    respond(req, nil, "不支援的請求：" .. tostring(req.command))
    return nil
  end
  local ok2, r = pcall(h, req)
  if not ok2 then
    respond(req, nil, tostring(r))
    return nil
  end
  return r
end

-- ---------------------------------------------------------------- 暫停

-- 在 base（相對於 stop，1 為 stop 本身）的框架暫停，處理請求直到繼續
local function stop(reason, base, text)
  while true do
    local info = getinfo(base, "S")
    if not info or info.what ~= "C" then break end
    base = base + 1
  end
  local total = count_from(2) -- 從 stop 到底部（count_from 自己是 1，stop 是 2）
  local from_bottom = {}
  for k = 0, total - base do from_bottom[k] = total - (base + k) end
  stopped = { from_bottom = from_bottom, depth = total - base + 1 }
  refs, nextref = {}, 1
  step, pause_req = nil, false
  event("stopped", { reason = reason, threadId = 1, allThreadsStopped = true, text = text, description = text })
  while true do
    local msg = wait()
    if msg == nil then
      reset()
      break
    end
    if dispatch(msg) == "resume" then break end
  end
  stopped = nil
  refs = {}
  update_hook()
end

-- 條件與記錄點在中斷點所在的框架求值（from_bottom：該框架距離堆疊底部的框架數）
local function should_break(set, line, from_bottom)
  local b = set and set[line]
  if not b then return false end
  stopped = { from_bottom = { [0] = from_bottom }, depth = 0 }
  local hit = true
  if b.cond and b.cond ~= "" then
    local v = evaluate(b.cond, 0)
    hit = v and true or false
  end
  if hit and b.log then
    local text = b.log:gsub("{(.-)}", function(e)
      local v, err = evaluate(e, 0)
      return err and ("{" .. err .. "}") or tostring(v)
    end)
    event("output", { category = "console", output = text .. "\n" })
    hit = false
  end
  stopped = nil
  return hit
end

-- 行鉤子（prelude 的鉤子在啟用行事件時呼叫）。層級：1 = M.line、2 = 鉤子、3 = 執行中的函式
-- 注意：呼叫 stop 不可寫成 return stop(...)，尾呼叫會少一層框架，層級就錯了
function M.line(line)
  if busy or not attached then return end
  if pause_req then
    stop("pause", 4)
    return
  end
  if step then
    local d = count_from(4)
    if step.kind == "in" or (step.kind == "over" and d <= step.depth) or (step.kind == "out" and d < step.depth) then
      stop("step", 4)
      return
    end
  end
  if bp_lines[line] then
    local info = getinfo(3, "S")
    local src = info.source or ""
    -- 執行中的函式距離底部：count_from(2) 從 M.line 數到底部，減去 M.line、鉤子、函式本身這 3 層
    if src:sub(1, 1) == "@" and should_break(bps[norm(src:sub(2))], line, count_from(2) - 3) then
      stop("breakpoint", 4)
    end
  end
end

-- 指令計數鉤子：執行中也能處理請求（例如暫停、修改中斷點）
function M.count()
  if busy then return end
  while true do
    local msg = poll()
    if msg == nil then return end
    if msg == "" then return reset() end
    dispatch(msg)
    if pause_req and attached then
      stop("pause", 4)
      return
    end
  end
end

-- 錯誤處理常式（prelude 的 __call）：錯誤時暫停在發生錯誤的地方。層級：1 = M.error、2 = 處理常式、3 起為錯誤發生處
function M.error(e)
  if attached and break_on_error and not busy then
    -- 處理常式不在鉤子裡執行：暫停期間擋住鉤子，除錯器自己的程式碼才不會被攔下
    busy = true
    stop("exception", 4, tostring(e))
    busy = false
  end
end

-- 一次呼叫（命令、事件）結束：清除單步
function M.finished()
  step, pause_req = nil, false
  update_hook()
end

-- 閒置時收到的訊息（Rust 呼叫）；nil 表示連線中斷
function M.handle(msg)
  if msg == nil then return reset() end
  busy = true
  dispatch(msg)
  busy = false
end

-- 重新載入腳本時把連線狀態帶到新的 Lua 狀態（VS Code 不會重送中斷點）
function M.export()
  if not attached then return nil end
  local list = array()
  for path, set in pairs(bps) do
    for line, b in pairs(set) do list[#list + 1] = { path = path, line = line, cond = b.cond, log = b.log } end
  end
  return encode({ seq = seq, bps = list, break_on_error = break_on_error })
end

function M.import(text)
  local ok, s = pcall(decode, text)
  if not ok or type(s) ~= "table" then return end
  attached, seq, break_on_error = true, s.seq or 0, s.break_on_error ~= false
  bps, bp_lines = {}, {}
  for _, b in ipairs(s.bps or {}) do
    bps[b.path] = bps[b.path] or {}
    bps[b.path][b.line] = { cond = b.cond, log = b.log }
    bp_lines[b.line] = true
  end
  update_hook()
  event("output", { category = "console", output = "已重新載入腳本\n" })
end

function M.output(text)
  if attached then event("output", { category = "stdout", output = text .. "\n" }) end
end

__attach_debugger(M)
__debugger = M
