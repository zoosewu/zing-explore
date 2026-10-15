-- 以 Lua 樣式（pattern）批次重新命名，並可復原上一次的重新命名
-- 取自 Directory Opus 腳本「RegexRenamePro」的核心：樣式取代、只改主檔名、名稱衝突加序號、復原記錄
-- Lua 樣式與正規表示式相近：%d 數字、%s 空白、%a 字母、. 任意字元、* + - ? 次數、( ) 擷取；取代字串用 %1 %2 代入擷取
-- 例：樣式 "^IMG_(%d+)"、取代 "相片 %1"：IMG_0012.jpg → 相片 0012.jpg
-- 安裝：複製到 zing.exe 旁的 scripts\，命令面板執行「重新載入 Lua 腳本」

-- 復原記錄（每行「新路徑<Tab>原路徑」，只保留最近一次）
local UNDO_DIR = path.join(os.getenv("LOCALAPPDATA") or os.getenv("TEMP") or ".", "zing-explore")
local UNDO_FILE = path.join(UNDO_DIR, "rename-undo.tsv")

local function exists(p)
  local f = io.open(p, "rb")
  if f then f:close() return true end
  return is_dir(p)
end

-- "a.txt" → "a", ".txt"；沒有副檔名時 → "a", ""
local function split_ext(name)
  local base, ext = name:match("^(.+)(%.[^.]*)$")
  if base then return base, ext end
  return name, ""
end

-- 名稱衝突時加上序號：a.txt → a (2).txt
local function unique(dir, name, taken)
  local base, ext = split_ext(name)
  local n, candidate = 1, name
  while taken[candidate:lower()] or exists(path.join(dir, candidate)) do
    n = n + 1
    candidate = ("%s (%d)%s"):format(base, n, ext)
  end
  return candidate
end

-- 算出每個項目的新名稱；回傳 { {from, to}, ... }（名稱不變的略過）或 nil 與錯誤訊息
local function plan(items, pattern, repl, whole_name)
  local renames, taken = {}, {}
  for _, p in ipairs(items) do
    local name = path.name(p)
    local base, ext = name, ""
    if not whole_name and not is_dir(p) then base, ext = split_ext(name) end
    local ok, new_base = pcall(string.gsub, base, pattern, repl)
    if not ok then return nil, "樣式有誤：" .. tostring(new_base):gsub("^.-:%d+: ", "") end
    local new = new_base .. ext
    if new ~= name and new ~= "" then
      -- 只改大小寫時不算衝突（Windows 的檔名不分大小寫）
      if new:lower() ~= name:lower() then new = unique(path.dir(p), new, taken) end
      taken[new:lower()] = true
      renames[#renames + 1] = { from = p, to = path.join(path.dir(p), new) }
    end
  end
  return renames
end

local function save_undo(done)
  if not is_dir(UNDO_DIR) then os.execute('mkdir "' .. UNDO_DIR .. '"') end
  local f = io.open(UNDO_FILE, "w")
  if not f then return end
  for _, r in ipairs(done) do f:write(r.to, "\t", r.from, "\n") end
  f:close()
end

local function yes(answer)
  answer = answer:lower()
  return answer == "是" or answer == "y" or answer == "yes" or answer == "1"
end

--- 以 Lua 樣式批次重新命名
function rename_pattern()
  local items = sel()
  if #items == 0 then return msg("請先選取要重新命名的項目") end

  local pattern = ask("樣式（Lua pattern，例：^IMG_(%d+)）", "(.+)")
  if not pattern or pattern == "" then return end
  local repl = ask("取代為（%1 %2 代入擷取的部分）", "%1")
  if not repl then return end
  local scope = ask("套用到：1 主檔名（保留副檔名）／2 完整名稱", "1")
  if not scope then return end

  local renames, err = plan(items, pattern, repl, scope == "2")
  if not renames then return msg(err or "樣式有誤") end
  if #renames == 0 then return msg("沒有名稱需要變更") end

  local first = renames[1]
  local sure = ask(("重新命名 %d 個項目，例如「%s」→「%s」。確定？（是／否）")
    :format(#renames, path.name(first.from), path.name(first.to)), "是")
  if not sure or not yes(sure) then return end

  local done, failed = {}, {}
  for _, r in ipairs(renames) do
    local ok, why = os.rename(r.from, r.to)
    if ok then
      done[#done + 1] = r
    else
      failed[#failed + 1] = path.name(r.from)
      log("重新命名失敗：", r.from, why)
    end
  end
  save_undo(done)

  if #failed == 0 then
    msg(("已重新命名 %d 個項目（可用「復原上一次的樣式重新命名」還原）"):format(#done))
  else
    msg(("已重新命名 %d 個，%d 個失敗（%s…，詳見 scripts\\zing.log）"):format(#done, #failed, failed[1]))
  end
end

--- 復原上一次的樣式重新命名
function rename_undo()
  local f = io.open(UNDO_FILE, "r")
  if not f then return msg("沒有可以復原的重新命名") end
  local entries = {}
  for line in f:lines() do
    local new, old = line:match("^(.-)\t(.*)$")
    if new then entries[#entries + 1] = { new = new, old = old } end
  end
  f:close()

  -- 倒序復原：先改的後還原，連鎖的名稱（a→b、b→c）才不會互相覆蓋
  local restored, failed = 0, 0
  for i = #entries, 1, -1 do
    local e = entries[i]
    local free = e.old:lower() == e.new:lower() or not exists(e.old) -- 只改大小寫時原名稱就是自己
    if free and os.rename(e.new, e.old) then
      restored = restored + 1
    else
      failed = failed + 1
      log("無法復原：", e.new, "→", e.old)
    end
  end
  os.remove(UNDO_FILE)
  if failed == 0 then
    msg(("已復原 %d 個項目"):format(restored))
  else
    msg(("已復原 %d 個，%d 個無法復原（檔案已移動或原名稱已被使用，詳見 scripts\\zing.log）"):format(restored, failed))
  end
end
