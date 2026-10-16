-- 整理選取的檔案到子資料夾：
-- - 依類型：依副檔名移到「圖片」「影片」「音樂」「文件」「壓縮檔」「程式」「其他」
-- - 依日期：依修改日期移到「年-月」（示範 stat()）
-- 移動經 zing 的檔案作業佇列（顯示進度、可暫停或取消，名稱衝突由 Windows 詢問）
-- 安裝：複製到 zing.exe 旁的 scripts\，命令面板執行「重新載入 Lua 腳本」

-- 分類與副檔名：依需要修改
local KINDS = {
  { "圖片", "jpg jpeg png gif bmp webp heic heif tif tiff svg ico raw cr2 nef arw dng psd" },
  { "影片", "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts 3gp" },
  { "音樂", "mp3 flac wav aac m4a ogg opus wma ape" },
  { "文件", "pdf doc docx xls xlsx ppt pptx odt ods odp txt md rtf csv epub" },
  { "壓縮檔", "zip 7z rar tar gz tgz bz2 xz iso cab" },
  { "程式", "exe msi msix appx bat cmd ps1" },
}
local OTHER = "其他"

-- 副檔名 → 分類名稱
local KIND_OF = {}
for _, k in ipairs(KINDS) do
  for ext in k[2]:gmatch("%S+") do KIND_OF[ext] = k[1] end
end

local function kind(p) return KIND_OF[path.ext(p)] or OTHER end

local function selected_files() return sel() | filter(function(p) return not is_dir(p) end) | list end

-- 依 order 的順序把每組檔案移到 base\組名（建立資料夾後排入檔案作業佇列）
local function move_groups(base, order, groups)
  for _, k in ipairs(order) do
    local dest = path.join(base, k)
    if not is_dir(dest) then os.execute('mkdir "' .. dest .. '"') end
    copy(groups[k], { to = dest, move = true })
  end
end

--- 依類型整理到子資料夾
function organize_by_type()
  local files = selected_files()
  if #files == 0 then return msg("請先選取要整理的檔案（資料夾不會移動）") end

  -- 分組，保持 KINDS 的順序
  local groups = {}
  for _, p in ipairs(files) do
    local k = kind(p)
    groups[k] = groups[k] or {}
    table.insert(groups[k], p)
  end
  local names = seq(KINDS) | map(function(k) return k[1] end) | list
  names = table.move(names, 1, #names, 1, {}) -- list 回傳的清單不可修改，複製一份再加上「其他」
  names[#names + 1] = OTHER

  local order = seq(names) | filter(function(k) return groups[k] ~= nil end) | list
  local summary = seq(order) | map(function(k) return k .. " " .. #groups[k] end) | list
  local base = ask(("移動 %d 個檔案（%s）到哪個資料夾下的分類子資料夾？")
    :format(#files, table.concat(summary, "、")), cwd())
  if not base or base == "" then return end

  move_groups(base, order, groups)
  msg(("已排入移動：%d 個檔案到 %d 個分類"):format(#files, #order))
end

-- 修改日期的「年-月」；讀不到時為「未知日期」
local function month(p)
  local s = stat(p)
  if not s or not s.modified then return "未知日期" end
  return tostring(os.date("%Y-%m", s.modified))
end

--- 依修改日期整理到「年-月」子資料夾
function organize_by_date()
  local files = selected_files()
  if #files == 0 then return msg("請先選取要整理的檔案（資料夾不會移動）") end

  local groups, order = {}, {}
  for _, p in ipairs(files) do
    local m = month(p)
    if not groups[m] then
      groups[m] = {}
      order[#order + 1] = m
    end
    table.insert(groups[m], p)
  end
  table.sort(order)

  local range = #order == 1 and order[1] or (order[1] .. " ～ " .. order[#order])
  local base = ask(("移動 %d 個檔案（%s，共 %d 個月）到哪個資料夾下的「年-月」子資料夾？")
    :format(#files, range, #order), cwd())
  if not base or base == "" then return end

  move_groups(base, order, groups)
  msg(("已排入移動：%d 個檔案到 %d 個月份資料夾"):format(#files, #order))
end
