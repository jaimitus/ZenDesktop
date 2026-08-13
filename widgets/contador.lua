-- Contador interactivo (ejemplo de widget "complicado")
--   * estado persistente: la tabla `state` sobrevive entre renders y clics
--   * interactividad: function click(x, y, w, h) al pulsar en el cuerpo
--   * primitivas nuevas: round_rect, circle, text_center, text_right, line

WIDTH = 240
HEIGHT = 150
TITLE = "Contador"

-- Si no hay estado previo (primer render), inicializarlo.
if state.count == nil then
    state.count = 0
    state.step = 1
end

-- Geometria compartida por render() y click(): si cambias los botones aqui,
-- se mueven en los dos sitios.
local function layout(w)
    local bw = (w - 60) / 2
    local bh = 34
    return {
        bw = bw, bh = bh,
        plus  = { x = 18,          y = 84, w = bw, h = bh },
        reset = { x = 18 + bw + 24, y = 84, w = bw, h = bh },
    }
end

local function in_rect(x, y, r)
    return x >= r.x and x <= r.x + r.w and y >= r.y and y <= r.y + r.h
end

function render(ctx)
    local w = ctx:width()
    local h = ctx:height()
    local L = layout(w)

    -- Fondo suave.
    ctx:fill_rect(0, 0, w, h, 0x0AFFFFFF)

    -- Contador grande centrado.
    local label = string.format("%d", state.count)
    if state.count == 0 then
        ctx:text_center(w / 2, 18, "Pulsa para contar", 15, 0x88FFFFFF)
    else
        ctx:text_center(w / 2, 10, label, 34, 0xFF4FC3F7)
    end

    -- Barra de progreso proporcional al contador (mod 100).
    ctx:progress(20, 62, w - 40, 8, (state.count % 100) / 100, 0xFF4FC3F7)

    -- Boton "+1" (rectangulo redondeado con borde).
    ctx:round_rect(L.plus.x, L.plus.y, L.plus.w, L.plus.h, 10, 0xFF1E5F8A, 1.5, 0xFF4FC3F7)
    ctx:text_center(L.plus.x + L.plus.w / 2, L.plus.y + 7, "+1", 15, 0xFFFFFFFF)

    -- Boton "Reset" (redondeado con borde tenue).
    ctx:round_rect(L.reset.x, L.reset.y, L.reset.w, L.reset.h, 10, 0xFF262626, 1.5, 0x66777777)
    ctx:text_center(L.reset.x + L.reset.w / 2, L.reset.y + 7, "Reset", 14, 0xAAFFFFFF)

    -- Una linea y un circulo decorativos.
    ctx:line(20, h - 10, w - 20, h - 10, 1, 0x22FFFFFF)
    ctx:circle(w - 16, h - 14, 3, 0xFF4FC3F7)
end

function click(x, y, w, h)
    local L = layout(w)
    if in_rect(x, y, L.plus) then
        state.count = state.count + state.step
        if state.count % 50 == 0 then
            app:notify(string.format("Llevas %d pulsaciones", state.count))
        end
    elseif in_rect(x, y, L.reset) then
        state.count = 0
    end
end
