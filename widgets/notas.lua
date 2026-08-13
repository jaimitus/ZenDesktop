-- ZenDesktop :: widget de ejemplo (notas)
-- Copialo a <carpeta de config>/widgets/notas.lua y anade a config.toml:
--
--   [[fences]]
--   id = "notas"
--   x = 100
--   y = 100
--   width = 260
--   height = 230
--   widget = "notas"
--
-- Edita la lista NOTES para poner tus propias tareas.

TITLE = "Notas"

local NOTES = {
    "Comprar leche",
    "Llamar al dentista",
    "Enviar el informe",
    "Pagar las facturas",
    "Leer 20 minutos",
    "Regar las plantas",
}

-- Cuantas tareas estan completadas (editalo y la barra se mueve).
local COMPLETED = 2

function render(ctx)
    local w = ctx:width()
    local h = ctx:height()

    ctx:fill_rect(0, 0, w, h, 0x00000000)

    -- Cabecera con la fecha del dia.
    local now = os.date("*t")
    local date = string.format("%02d/%02d/%04d", now.day, now.month, now.year)
    ctx:text(16, 12, "Notas", 20, 0xFFFFFFFF)
    ctx:text(18, 36, date, 11, 0x88FFFFFF)

    -- Separador.
    ctx:fill_rect(16, 54, w - 32, 1, 0x33FFFFFF)

    -- Lista de tareas (con numero).
    local y = 68
    local row = 24
    for i, note in ipairs(NOTES) do
        if y + 18 > h - 34 then
            break
        end
        ctx:text(16, y, string.format("%d.", i), 13, 0xFF38BDF8)
        ctx:text(36, y, note, 13, 0xEEFFFFFF)
        y = y + row
    end

    -- Progreso de tareas completadas.
    local total = #NOTES
    local ratio = 0
    if total > 0 then
        ratio = COMPLETED / total
    end
    ctx:progress(16, h - 26, w - 32, 5, ratio, 0xFF38BDF8)
    ctx:text(16, h - 17, string.format("%d/%d completadas", COMPLETED, total), 10, 0x88FFFFFF)
end
