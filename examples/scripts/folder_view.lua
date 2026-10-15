-- 資料夾檢視規則：示範事件函式 on_navigate 與 thumbnail
-- - 進入相片、影片類的資料夾時自動切換成縮圖檢視
-- - 資料夾的縮圖使用其中的封面圖片（folder.jpg、cover.jpg、poster.jpg…，音樂與影片資料夾常見）
-- 事件函式每個名稱只能有一個：若其他腳本也定義了 on_navigate 或 thumbnail，請合併成一個
-- 安裝：複製到 zing.exe 旁的 scripts\，命令面板執行「重新載入 Lua 腳本」

-- 路徑符合任一 Lua 樣式（不分大小寫）就切換成縮圖檢視
local THUMB_FOLDERS = {
  "\\pictures\\", "\\pictures$",
  "\\photos?\\", "\\photos?$",
  "\\相片", "\\圖片", "\\照片",
  "\\videos\\", "\\videos$", "\\影片",
  "\\dcim\\", "\\dcim$",
}

-- 資料夾封面的檔名，依序尋找
local COVERS = { "folder.jpg", "cover.jpg", "cover.png", "poster.jpg", "front.jpg", "albumart.jpg" }

local function wants_thumbs(dir)
  local p = dir:lower()
  for _, pat in ipairs(THUMB_FOLDERS) do
    if p:find(pat) then return true end
  end
  return false
end

local function exists(p)
  local f = io.open(p, "rb")
  if f then f:close() return true end
  return false
end

function on_navigate(dir)
  if wants_thumbs(dir) then view.mode("thumbnails") end
end

function thumbnail(p)
  if not is_dir(p) then return nil end
  for _, name in ipairs(COVERS) do
    local cover = path.join(p, name)
    if exists(cover) then return cover end
  end
  return nil
end
