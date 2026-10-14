-- 批次解壓縮：把選取的壓縮檔解壓縮到指定的資料夾
-- 使用 Windows 內建的 tar.exe（bsdtar），支援 zip、7z、rar、tar（含 gz／bz2／xz）、iso、cab
-- 安裝：複製到 zing.exe 旁的 scripts\，命令面板執行「重新載入 Lua 腳本」

local ARCHIVE = freeze { zip = true, ["7z"] = true, rar = true, tar = true, gz = true, tgz = true,
                         bz2 = true, xz = true, iso = true, cab = true }

-- 用系統的 tar.exe，不用 PATH 上的（Git 附的 GNU tar 不支援 zip）
local TAR = (os.getenv("SystemRoot") or "C:\\Windows") .. "\\System32\\tar.exe"

local function is_archive(p) return not is_dir(p) and ARCHIVE[path.ext(p)] == true end

-- 子資料夾名稱：a.zip → a，a.tar.gz → a
local function folder_name(p) return (path.stem(p):gsub("%.tar$", "")) end

-- 解壓縮一個檔案到 dest；回傳成功與否。run 的 wait 會等 tar 結束（等待不計入 CPU 時間上限，主控台視窗不會出現）
local function extract_one(archive, dest)
  if not is_dir(dest) then os.execute('mkdir "' .. dest .. '"') end
  return run{ TAR, "-xf", archive, "-C", dest, wait = true } == 0
end

local function yes(answer)
  answer = answer:lower()
  return answer == "是" or answer == "y" or answer == "yes" or answer == "1"
end

--- 批次解壓縮到指定資料夾
function extract_to()
  local picked = sel()
  if #picked == 0 then return msg("請先選擇要解壓縮的壓縮檔") end

  local archives = picked | filter(is_archive) | list
  if #archives == 0 then return msg("選擇的檔案中沒有壓縮檔") end

  local dest = ask("解壓縮 " .. #archives .. " 個壓縮檔到：", cwd())
  if not dest or dest == "" then return end

  local answer = ask("為每個壓縮檔建立子資料夾？（是／否）", "是")
  if not answer then return end
  local sub = yes(answer)

  local target = function(a) return sub and path.join(dest, folder_name(a)) or dest end
  local failed = archives | filter(function(a)
    msg("解壓縮：" .. path.name(a))
    return not extract_one(a, target(a))
  end) | list

  if #failed == 0 then
    msg("解壓縮完成：" .. #archives .. " 個壓縮檔 → " .. dest)
  else
    log("解壓縮失敗：", table.concat(failed, "; "))
    msg("解壓縮完成，" .. #failed .. " 個失敗（" .. path.name(failed[1]) .. "…，詳見 scripts\\zing.log）")
  end
end
