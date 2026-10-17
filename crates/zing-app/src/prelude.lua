-- zing-explore 內建函式庫：惰性串流、不可變、純函式、組合。說明見 docs/lua.md（由 scriptapi.rs 產生）。
-- 由 Rust 提供：__invoke(名稱, 位置參數, 旗標)、__msg、__exec、__ask、__pick_folder、__log、__is_dir、__stat、
-- __clipboard_get、__clipboard_set、__expired、__tr、
-- __spawn、__shell、__popen_open/read/write/close、__commands、__ctx
-- Rust 提供的函式不會丟錯；錯誤一律在 Lua 這邊丟出（見 script.rs 開頭）

-- 介面文字的翻譯（原文為繁體中文，見 zing_core::i18n）；args 代入 {名稱}
local function tr(s, args)
  s = __tr(s)
  if args then
    s = s:gsub("{(%w+)}", function(k)
      if args[k] ~= nil then return tostring(args[k]) end
    end)
  end
  return s
end

-- 鉤子：CPU 時間上限（每 10000 個指令問一次 Rust，超過時中止）；除錯器連線時另外處理行事件與執行中收到的請求。
-- 鉤子是每個協程各自設定的，新建的協程也要設
local expired, sethook = __expired, debug.sethook
local dbg = nil        -- debugger.lua（__attach_debugger）
local mask = ""        -- 除錯器需要時為 "l"（每行觸發）
local main = coroutine.running()
local threads = setmetatable({}, { __mode = "k" })
local function hook(event, line)
  if event == "count" then
    if expired() then error(tr("執行超過 CPU 時間上限，已中止"), 2) end
    if dbg then dbg.count() end
  elseif dbg then
    dbg.line(line)
  end
end
local function apply(co) sethook(co, hook, mask, 10000) end
apply(main)
function __attach_debugger(d) dbg = d end
function __set_line_hook(on)
  local m = on and "l" or ""
  if m == mask then return end
  mask = m
  apply(main)
  for co in pairs(threads) do
    if coroutine.status(co) ~= "dead" then apply(co) end
  end
end
do
  local create, resume = coroutine.create, coroutine.resume
  coroutine.create = function(f)
    local co = create(f)
    threads[co] = true
    apply(co)
    return co
  end
  coroutine.wrap = function(f)
    local co = coroutine.create(f)
    return function(...)
      local r = table.pack(resume(co, ...))
      if not r[1] then error(r[2], 2) end
      return table.unpack(r, 2, r.n)
    end
  end
end

-- ---------------------------------------------------------------- 不可變

local Frozen = {}

local function readonly()
  error(tr("不可變的資料不能修改"), 2)
end

--- 回傳不可變的唯讀版本
function freeze(t)
  if getmetatable(t) == Frozen then
    return t
  end
  return setmetatable({}, {
    __index = t,
    __newindex = readonly,
    __len = function() return #t end,
    __pairs = function() return next, t, nil end,
    __frozen = t,
    __metatable = Frozen,
  })
end

-- ---------------------------------------------------------------- 惰性串流

local Seq = {}
Seq.__index = Seq

-- 每個 Seq 保存一個「產生迭代器」的函式：每次取值都從頭開始，所以可以重複使用、不會被消耗
local function make(factory)
  return setmetatable({ _iter = factory }, Seq)
end

local function from(src)
  if getmetatable(src) == Seq then
    return src
  end
  local t = type(src)
  if t == "table" then
    return make(function()
      local i = 0
      return function()
        i = i + 1
        return src[i]
      end
    end)
  elseif t == "function" then
    -- 迭代函式只能走一次
    return make(function() return src end)
  end
  error(tr("seq 需要清單、Seq 或迭代函式"), 3)
end

--- 建立惰性串流
function seq(src)
  return from(src)
end

function Seq:map(f)
  local up = self._iter
  return make(function()
    local it = up()
    return function()
      local v = it()
      if v ~= nil then return f(v) end
    end
  end)
end

function Seq:filter(p)
  local up = self._iter
  return make(function()
    local it = up()
    return function()
      local v = it()
      while v ~= nil and not p(v) do v = it() end
      return v
    end
  end)
end

function Seq:take(n)
  local up = self._iter
  return make(function()
    local it, i = up(), 0
    return function()
      i = i + 1
      if i <= n then return it() end
    end
  end)
end

function Seq:skip(n)
  local up = self._iter
  return make(function()
    local it = up()
    for _ = 1, n do
      if it() == nil then break end
    end
    return it
  end)
end

function Seq:take_while(p)
  local up = self._iter
  return make(function()
    local it, done = up(), false
    return function()
      if done then return nil end
      local v = it()
      if v == nil or not p(v) then
        done = true
        return nil
      end
      return v
    end
  end)
end

function Seq:sort_by(key)
  local up = self._iter
  return make(function()
    local items = {}
    for v in up() do items[#items + 1] = v end
    table.sort(items, function(a, b) return key(a) < key(b) end)
    local i = 0
    return function()
      i = i + 1
      return items[i]
    end
  end)
end

function Seq:list()
  local out = {}
  for v in self._iter() do out[#out + 1] = v end
  return freeze(out)
end

function Seq:each(f)
  for v in self._iter() do f(v) end
end

function Seq:reduce(f, init)
  local acc = init
  for v in self._iter() do acc = f(acc, v) end
  return acc
end

function Seq:count()
  local n = 0
  for _ in self._iter() do n = n + 1 end
  return n
end

function Seq:first()
  return self._iter()()
end

-- ---------------------------------------------------------------- 管線：x | f 等於 f(x)

local function bor(a, b)
  if type(b) ~= "function" then
    error(tr("| 的右邊必須是函式（管線步驟）"), 2)
  end
  return b(a)
end
Seq.__bor = bor
-- 所有函式共用一個 metatable：讓清單、字串等任何值都能用 | 傳給函式
debug.setmetatable(function() end, { __bor = bor })

local function stage(method)
  return function(...)
    local args = table.pack(...)
    return function(s) return from(s)[method](from(s), table.unpack(args, 1, args.n)) end
  end
end

map = stage("map")
filter = stage("filter")
take = stage("take")
skip = stage("skip")
take_while = stage("take_while")
sort_by = stage("sort_by")
each = stage("each")
reduce = stage("reduce")

function list(s) return from(s):list() end
function count(s) return from(s):count() end
function first(s) return from(s):first() end

-- ---------------------------------------------------------------- 組合

function compose(...)
  local fs = table.pack(...)
  return function(...)
    local r = table.pack(...)
    for i = fs.n, 1, -1 do r = table.pack(fs[i](table.unpack(r, 1, r.n))) end
    return table.unpack(r, 1, r.n)
  end
end

function pipe(x, ...)
  for _, f in ipairs({ ... }) do x = f(x) end
  return x
end

function partial(f, ...)
  local pre = table.pack(...)
  return function(...)
    local all = { table.unpack(pre, 1, pre.n) }
    local rest = table.pack(...)
    for i = 1, rest.n do all[pre.n + i] = rest[i] end
    return f(table.unpack(all, 1, pre.n + rest.n))
  end
end

-- ---------------------------------------------------------------- 路徑（純函式）

path = {}

function path.name(p)
  return (p:gsub("[\\/]+$", ""):match("([^\\/]*)$"))
end

function path.stem(p)
  local n = path.name(p)
  return n:match("^(.+)%.[^.]*$") or n
end

function path.ext(p)
  local n = path.name(p)
  local e = n:match("^.+%.([^.]*)$")
  return e and e:lower() or ""
end

function path.dir(p)
  local d = p:gsub("[\\/]+$", ""):match("^(.*)[\\/][^\\/]*$")
  if d and d:match("^%a:$") then return d .. "\\" end
  return d or ""
end

function path.join(...)
  local parts = { ... }
  local out = ""
  for _, x in ipairs(parts) do
    if out == "" then
      out = x
    elseif out:sub(-1) == "\\" or out:sub(-1) == "/" then
      out = out .. x
    else
      out = out .. "\\" .. x
    end
  end
  return out
end

-- ---------------------------------------------------------------- 狀態與互動

local function ctx() return __ctx or { cwd = "", sel = {} } end

function cwd() return ctx().cwd end
function focus() return ctx().focus end

function msg(text) __msg(tostring(text)) end
function exec(script) __exec(tostring(script)) end
function ask(prompt, default) return __ask(tostring(prompt), default and tostring(default) or "") end
function pick_folder(title, default)
  return __pick_folder(title and tostring(title) or "", default and tostring(default) or "")
end
function is_dir(p) return __is_dir(p) end
function stat(p) return __stat(tostring(p)) end
function clipboard(text)
  if text == nil then return __clipboard_get() end
  return __clipboard_set(tostring(text))
end

function log(...)
  local parts = table.pack(...)
  for i = 1, parts.n do parts[i] = tostring(parts[i]) end
  local text = table.concat(parts, " ", 1, parts.n)
  __log(text)
  if dbg then dbg.output(text) end
end

-- ---------------------------------------------------------------- 內建命令

-- 參數轉成位置參數與旗標：字串、數字是位置參數；表格的整數鍵是位置參數（清單展開），字串鍵是 --鍵=值
local function args_of(...)
  local pos, flags = {}, {}
  local function add(v)
    local t = type(v)
    if t == "string" or t == "number" then
      pos[#pos + 1] = tostring(v)
    elseif getmetatable(v) == Seq then
      for x in v._iter() do add(x) end
    elseif t == "table" then
      for _, x in ipairs(v) do add(x) end
      for k, x in pairs(v) do
        if type(k) == "string" then
          if x == true then
            flags[k] = ""
          elseif x ~= false and x ~= nil then
            flags[k] = tostring(x)
          end
        end
      end
    elseif v ~= nil then
      error(tr("命令參數只能是字串、數字、清單或表格"), 3)
    end
  end
  for i = 1, select("#", ...) do add((select(i, ...))) end
  return pos, flags
end

local function command(name)
  return function(...)
    local pos, flags = args_of(...)
    __invoke(name, pos, flags)
  end
end

-- 命名空間：已有同名函式（sel、filter）時變成可呼叫的表格，函式與命令都能用
local function namespace(name)
  local cur = _G[name]
  if type(cur) == "table" then return cur end
  local ns = {}
  if type(cur) == "function" then
    setmetatable(ns, { __call = function(_, ...) return cur(...) end })
  end
  _G[name] = ns
  return ns
end

-- 選取：不可變清單（命令開始時的快照）
sel = function() return freeze(ctx().sel) end

for _, full in ipairs(__commands) do
  local ns, field = full:match("^([%w_]+)%.([%w_]+)$")
  if ns then
    namespace(ns)[field] = command(full)
  else
    _G[full] = command(full)
  end
end

function go(p) __invoke("nav.goto", { tostring(p) }, {}) end

-- run：主控台程式預設不顯示視窗（console = true 時顯示）；wait = true 時在腳本執行緒等程式結束並回傳結束代碼
do
  local spawn = __spawn
  run = function(...)
    local pos, flags = args_of(...)
    if flags.wait then return spawn(pos, flags.console ~= nil, ctx().cwd) end
    __invoke("run", pos, flags)
  end
end

-- os.execute、io.popen：經 cmd.exe 在目前資料夾執行，不顯示主控台視窗（標準版本會閃出主控台視窗）
do
  local shell, open, read, write, close = __shell, __popen_open, __popen_read, __popen_write, __popen_close
  local function status(code)
    if code == nil then return nil, "exit", -1 end
    if code == 0 then return true, "exit", 0 end
    return nil, "exit", code
  end
  os.execute = function(cmd)
    if cmd == nil then return true end
    local code, err = shell(tostring(cmd), ctx().cwd)
    if code == nil and err then return nil, err end
    return status(code)
  end
  -- 讀取模式邊執行邊讀；寫入模式寫進命令的標準輸入。close 時等命令結束並回傳結束狀態
  io.popen = function(cmd, mode)
    mode = mode or "r"
    if mode ~= "r" and mode ~= "w" then error(tr("io.popen 的模式只能是 \"r\" 或 \"w\""), 2) end
    local id, err = open(tostring(cmd), mode, ctx().cwd)
    if not id then return nil, err end
    local f, closed = {}, false
    function f:read(...)
      local n = select("#", ...)
      if n == 0 then return read(id, "l") end
      local r = {}
      for i = 1, n do
        r[i] = read(id, (select(i, ...)))
        if r[i] == nil then return table.unpack(r, 1, i) end
      end
      return table.unpack(r, 1, n)
    end
    function f:lines(...)
      local fmts = table.pack(...)
      if fmts.n == 0 then return function() return read(id, "l") end end
      return function() return f:read(table.unpack(fmts, 1, fmts.n)) end
    end
    function f:write(...)
      for i = 1, select("#", ...) do
        if not write(id, (select(i, ...))) then return nil, tr("寫入失敗") end
      end
      return f
    end
    function f:flush() return f end
    function f:setvbuf() return true end
    function f:close()
      if closed then return nil, tr("已經關閉") end
      closed = true
      return status(close(id, true))
    end
    -- 沒有 close 就被回收：放掉程序（不等待）
    return setmetatable(f, {
      __name = "FILE*",
      __close = function() if not closed then f:close() end end,
      __gc = function() if not closed then closed = true; close(id, false) end end,
    })
  end
end

-- ---------------------------------------------------------------- 給 Rust 用

-- 呼叫並附上呼叫堆疊
-- 除錯器連線時，錯誤發生的當下（堆疊還在）先暫停；呼叫結束時清除單步
function __call(f, ...)
  local r = table.pack(xpcall(f, function(e)
    if dbg then dbg.error(e) end
    return debug.traceback(e, 2)
  end, ...))
  if dbg then dbg.finished() end
  return table.unpack(r, 1, r.n)
end

-- REPL：把結果轉成一行文字
function __show(v)
  if getmetatable(v) == Seq then v = v:list() end
  if type(v) == "table" then
    local parts = {}
    for i, x in ipairs(v) do
      if i > 20 then
        parts[#parts + 1] = tr("…（共 {n} 項）", { n = #v })
        break
      end
      parts[#parts + 1] = tostring(x)
    end
    return "{ " .. table.concat(parts, ", ") .. " }"
  end
  return tostring(v)
end
