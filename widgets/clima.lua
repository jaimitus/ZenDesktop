-- ZenDesktop :: widget de ejemplo (clima)
-- Copialo a <carpeta de config>/widgets/clima.lua y anade a config.toml:
--
--   [[fences]]
--   id = "clima"
--   x = 100
--   y = 100
--   width = 260
--   height = 160
--   widget = "clima"
--
-- Usa Open-Meteo (sin API key). Cambia LAT/LON a tu ciudad.

TITLE = "Clima"

local LAT = 40.42  -- Madrid
local LON = -3.70
local COLOR = 0xFF38BDF8

local function weather_label(code)
    if code == 0 then return "Despejado" end
    if code <= 3 then return "Parcialmente nublado" end
    if code <= 48 then return "Niebla" end
    if code <= 57 then return "Llovizna" end
    if code <= 67 then return "Lluvia" end
    if code <= 77 then return "Nieve" end
    if code <= 82 then return "Chubascos" end
    if code <= 86 then return "Chubascos de nieve" end
    return "Tormenta"
end

function render(ctx)
    local w = ctx:width()
    local h = ctx:height()
    ctx:fill_rect(0, 0, w, h, 0x00000000)

    local url = string.format(
        "https://api.open-meteo.com/v1/forecast?latitude=%.2f&longitude=%.2f&current_weather=true",
        LAT, LON)

    -- nil mientras la primera descarga esta en vuelo (la app repinta sola).
    local data = http:get_json(url)
    if data == nil then
        ctx:text(16, h * 0.30, "Cargando clima...", 16, 0x88FFFFFF)
        return
    end

    local cw = data["current_weather"] or {}
    local temp = cw["temperature"] or 0
    local wind = cw["windspeed"] or 0
    local code = cw["weathercode"] or 0

    ctx:text(16, 12, string.format("%.0f°C", temp), 34, COLOR)
    ctx:text(16, 58, weather_label(code), 15, 0xEEFFFFFF)
    ctx:text(16, 84, string.format("Viento: %.0f km/h", wind), 12, 0xAAFFFFFF)
end
