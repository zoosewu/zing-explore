-- 依類型整理：把選取的檔案依副檔名移到「圖片」「影片」「音樂」「文件」「壓縮檔」「程式」「其他」子資料夾
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

--- 依類型整理到子資料夾
function organize_by_type()
  local files = sel() | filter(function(p) return not is_dir(p) end) | list
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

  local summary = seq(names)
    | filter(function(k) return groups[k] ~= nil end)
    | map(function(k) return k .. " " .. #groups[k] end)
    | list
  local base = ask(("移動 %d 個檔案（%s）到哪個資料夾下的分類子資料夾？")
    :format(#files, table.concat(summary, "、")), cwd())
  if not base or base == "" then return end

  for _, k in ipairs(names) do
    if groups[k] then
      local dest = path.join(base, k)
      if not is_dir(dest) then os.execute('mkdir "' .. dest .. '"') end
      copy(groups[k], { to = dest, move = true })
    end
  end
  msg(("已排入移動：%d 個檔案到 %d 個分類"):format(#files, #summary))
end
