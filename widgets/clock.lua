-- ZenDesktop :: widget de ejemplo (reloj)
-- Copialo a <carpeta de config>/widgets/clock.lua y anade a config.toml:
--
--   [[fences]]
--   id = "clock"
--   x = 100
--   y = 100
--   width = 300
--   height = 150
--   widget = "clock"

TITLE = "Reloj"

function render(ctx)
    local w = ctx:width()
    local h = ctx:height()

    -- Fondo sutil.
    ctx:fill_rect(0, 0, w, h, 0x00000000)

    local now = os.date("*t")
    local time = string.format("%02d:%02d:%02d", now.hour, now.min, now.sec)
    local date = string.format("%02d/%02d/%04d", now.day, now.month, now.year)

    ctx:text(24, h * 0.22, time, 44, 0xFFFFFFFF)
    ctx:text(26, h * 0.60, date, 18, 0x88FFFFFF)

    -- Barra de segundos del minuto en curso.
    ctx:progress(24, h * 0.80, w - 48, 6, now.sec / 60, 0xFF38BDF8)
end
