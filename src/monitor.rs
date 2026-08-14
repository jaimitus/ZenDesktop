//! monitor.rs — Widget "Monitor del sistema": CPU, RAM, GPU y bateria en una caja.
//!
//! Lecturas en vivo ultraligeras (GetSystemTimes / GlobalMemoryStatusEx /
//! DXGI VideoMemory / GetSystemPowerStatus), muestreadas una vez por segundo
//! con buffer circular de historial para graficas tipo sparkline.

use std::collections::VecDeque;
use std::sync::Mutex;

use windows::core::Interface;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1,
    DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
};
use windows::Win32::System::Power::GetSystemPowerStatus;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

const HISTORY_LEN: usize = 24;

/// Instantanea de las lecturas del sistema para el widget.
#[derive(Clone, Debug, Default)]
pub struct MonitorStats {
    /// Uso de CPU, 0.0..=100.0 (delta entre muestras; 0 en la primera).
    pub cpu_percent: f32,
    /// Uso de RAM, 0.0..=100.0.
    pub ram_percent: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    /// Carga de bateria, 0..=100 (255 = desconocido).
    pub battery_percent: u8,
    /// true si el equipo no tiene bateria (escritorio).
    pub no_battery: bool,
    /// true si esta desenchufado (funcionando con bateria).
    pub on_battery: bool,

    // GPU
    pub has_gpu: bool,
    pub gpu_name: String,
    pub gpu_vram_percent: f32,
    pub gpu_vram_used_gb: f32,
    pub gpu_vram_total_gb: f32,

    // Historial para sparklines (ultimos 24 puntos de muestreo)
    pub cpu_history: Vec<f32>,
    pub ram_history: Vec<f32>,
    pub gpu_history: Vec<f32>,
}

#[derive(Default)]
struct HistoryState {
    cpu: VecDeque<f32>,
    ram: VecDeque<f32>,
    gpu: VecDeque<f32>,
}

static LAST_TIMES: Mutex<Option<(u64, u64)>> = Mutex::new(None);
static HISTORY: Mutex<Option<HistoryState>> = Mutex::new(None);

fn ft_to_u64(t: &FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

/// Muestrea la GPU primaria via DXGI (nombre, uso de VRAM y total).
fn sample_gpu() -> Option<(String, f32, f32, f32)> {
    unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().ok()?;
        let mut idx = 0;
        let mut best: Option<(String, f32, f32, f32, f32)> = None; // (name, used, total, percent, dedicated)

        while let Ok(adapter) = factory.EnumAdapters1(idx) {
            idx += 1;
            let desc = match adapter.GetDesc1() {
                Ok(d) => d,
                Err(_) => continue,
            };
            // Ignorar adaptadores software basicos (Microsoft Basic Render Driver)
            if (desc.Flags & 2) != 0 {
                continue;
            }

            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len]).trim().to_string();
            let dedicated_gb = desc.DedicatedVideoMemory as f32 / (1024.0 * 1024.0 * 1024.0);

            let mut used_gb = 0.0_f32;
            let mut total_gb = dedicated_gb;
            let mut percent = 0.0_f32;

            if let Ok(adapter3) = adapter.cast::<IDXGIAdapter3>() {
                let mut mem = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                if adapter3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut mem).is_ok() {
                    used_gb = mem.CurrentUsage as f32 / (1024.0 * 1024.0 * 1024.0);
                    let budget_gb = mem.Budget as f32 / (1024.0 * 1024.0 * 1024.0);
                    if total_gb < 0.1 {
                        total_gb = budget_gb;
                    }
                    if total_gb > 0.0 {
                        percent = (used_gb / total_gb * 100.0).clamp(0.0, 100.0);
                    }
                }
            }

            let is_better = match best {
                None => true,
                Some((_, _, _, _, prev_ded)) => dedicated_gb > prev_ded,
            };
            if is_better {
                best = Some((name, used_gb, total_gb, percent, dedicated_gb));
            }
        }

        best.map(|(name, used, total, percent, _)| (name, used, total, percent))
    }
}

/// Lee el estado actual del sistema y actualiza el historial para las graficas.
pub fn sample() -> MonitorStats {
    let mut stats = MonitorStats::default();

    // RAM: GlobalMemoryStatusEx da el porcentaje directamente.
    unsafe {
        let mut ms = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut ms).is_ok() {
            stats.ram_percent = ms.dwMemoryLoad as f32;
            stats.ram_total_gb = ms.ullTotalPhys as f32 / (1024.0 * 1024.0 * 1024.0);
            let avail_gb = ms.ullAvailPhys as f32 / (1024.0 * 1024.0 * 1024.0);
            stats.ram_used_gb = (stats.ram_total_gb - avail_gb).max(0.0);
        }
    }

    // CPU: delta entre muestras.
    unsafe {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
            let idle_t = ft_to_u64(&idle);
            let total_t = ft_to_u64(&kernel) + ft_to_u64(&user);
            if let Ok(mut guard) = LAST_TIMES.lock() {
                if let Some((prev_idle, prev_total)) = *guard {
                    let didle = idle_t.saturating_sub(prev_idle);
                    let dtotal = total_t.saturating_sub(prev_total);
                    if dtotal > 0 {
                        stats.cpu_percent =
                            (100.0 * (1.0 - didle as f32 / dtotal as f32)).clamp(0.0, 100.0);
                    }
                }
                *guard = Some((idle_t, total_t));
            }
        }
    }

    // GPU: DXGI Video Memory
    if let Some((name, used_gb, total_gb, vram_pct)) = sample_gpu() {
        stats.has_gpu = true;
        stats.gpu_name = name;
        stats.gpu_vram_used_gb = used_gb;
        stats.gpu_vram_total_gb = total_gb;
        stats.gpu_vram_percent = vram_pct;
    }

    // Bateria: BatteryFlag bit 7 (0x80) = no hay bateria.
    unsafe {
        let mut ps = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut ps).is_ok() {
            stats.no_battery = ps.BatteryFlag & 0x80 != 0;
            stats.on_battery = !stats.no_battery && ps.ACLineStatus == 0;
            stats.battery_percent = ps.BatteryLifePercent;
        }
    }

    // Actualizar historial circular para sparklines
    if let Ok(mut guard) = HISTORY.lock() {
        let hist = guard.get_or_insert_with(HistoryState::default);
        
        hist.cpu.push_back(stats.cpu_percent);
        while hist.cpu.len() > HISTORY_LEN {
            hist.cpu.pop_front();
        }
        
        hist.ram.push_back(stats.ram_percent);
        while hist.ram.len() > HISTORY_LEN {
            hist.ram.pop_front();
        }

        let gpu_val = if stats.has_gpu { stats.gpu_vram_percent } else { 0.0 };
        hist.gpu.push_back(gpu_val);
        while hist.gpu.len() > HISTORY_LEN {
            hist.gpu.pop_front();
        }

        stats.cpu_history = hist.cpu.iter().copied().collect();
        stats.ram_history = hist.ram.iter().copied().collect();
        stats.gpu_history = hist.gpu.iter().copied().collect();
    }

    stats
}
