-- SHA-256 校驗碼：計算選取檔案的雜湊並寫成 .sha256 檔，或以 .sha256 檔驗證檔案
-- 使用 Windows 內建的 certutil.exe；.sha256 檔與 sha256sum 相同格式（「雜湊  檔名」），Linux 可用 sha256sum -c 驗證
-- 示範以 io.popen 讀取外部程式的輸出（不顯示主控台視窗；等待輸出不計入 CPU 時間上限）與 clipboard()
-- 安裝：複製到 zing.exe 旁的 scripts\，命令面板執行「重新載入 Lua 腳本」

local EMPTY = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" -- 空檔案的 SHA-256

local function size(p)
  local f = io.open(p, "rb")
  if not f then return nil end
  local n = f:seek("end")
  f:close()
  return n
end

-- 回傳小寫的十六進位雜湊；失敗時回傳 nil
local function sha256(p)
  local n = size(p)
  if not n then return nil end
  if n == 0 then return EMPTY end -- 舊版 certutil 不接受空檔案
  local out = io.popen('certutil -hashfile "' .. p .. '" SHA256')
  if not out then return nil end
  local hash
  for line in out:lines() do
    -- 輸出依系統語言而不同，只找 64 個十六進位字元的那一行（舊版 certutil 以空白分隔）
    local h = line:gsub("%s", "")
    if not hash and #h == 64 and h:match("^%x+$") then hash = h:lower() end
  end
  out:close()
  return hash
end

local function is_file(p) return not is_dir(p) end

--- 計算 SHA-256 並寫成 .sha256 檔
function sha256_create()
  local files = sel() | filter(is_file) | list
  if #files == 0 then return msg("請先選取要計算的檔案") end

  local lines, failed = {}, {}
  for i, p in ipairs(files) do
    msg(("計算 SHA-256（%d/%d）：%s"):format(i, #files, path.name(p)))
    local h = sha256(p)
    if h then
      lines[#lines + 1] = h .. "  " .. path.name(p)
    else
      failed[#failed + 1] = path.name(p)
    end
  end
  if #lines == 0 then return msg("無法計算 SHA-256（" .. failed[1] .. "）") end

  local default = #files == 1 and path.name(files[1]) .. ".sha256" or path.name(cwd()) .. ".sha256"
  local name = ask("寫入校驗檔（在目前資料夾）", default)
  if not name or name == "" then return end
  local target = path.join(cwd(), name)
  local f, err = io.open(target, "wb") -- 換行用 LF（sha256sum 的格式）
  if not f then return msg("無法寫入 " .. name .. "：" .. tostring(err)) end
  f:write(table.concat(lines, "\n"), "\n")
  f:close()

  if #failed == 0 and #files == 1 then
    -- 只有一個檔案：雜湊也放到剪貼簿，方便貼到其他地方比對
    clipboard(lines[1]:sub(1, 64))
    msg(("已寫入 %s，雜湊已複製到剪貼簿：%s"):format(name, lines[1]:sub(1, 64)))
  elseif #failed == 0 then
    msg(("已寫入 %s（%d 個檔案）"):format(name, #lines))
  else
    msg(("已寫入 %s，%d 個檔案無法讀取（%s…）"):format(name, #failed, failed[1]))
  end
end

--- 以 .sha256 檔驗證檔案
function sha256_verify()
  local lists = sel() | filter(function(p) return path.ext(p) == "sha256" end) | list
  if #lists == 0 then return msg("請先選取 .sha256 校驗檔") end

  local ok, bad, missing, total = 0, {}, {}, 0
  for _, sums in ipairs(lists) do
    local dir = path.dir(sums)
    for line in io.lines(sums) do
      -- 「雜湊  檔名」或「雜湊 *檔名」（二進位模式）
      local want, name = line:gsub("^\239\187\191", ""):match("^(%x+)%s+%*?(.-)%s*$") -- 略過 UTF-8 BOM
      if want and name ~= "" then
        total = total + 1
        msg(("驗證（%d）：%s"):format(total, name))
        local got = sha256(path.join(dir, name))
        if not got then
          missing[#missing + 1] = name
        elseif got == want:lower() then
          ok = ok + 1
        else
          bad[#bad + 1] = name
        end
      end
    end
  end

  if #bad == 0 and #missing == 0 then
    msg(("驗證通過：%d 個檔案都相符"):format(ok))
  else
    for _, n in ipairs(bad) do log("SHA-256 不符：", n) end
    for _, n in ipairs(missing) do log("找不到檔案：", n) end
    msg(("驗證失敗：%d 個相符、%d 個不符、%d 個找不到（%s…，詳見 scripts\\zing.log）")
      :format(ok, #bad, #missing, bad[1] or missing[1]))
  end
end
