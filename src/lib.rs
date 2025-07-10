use std::str::FromStr as _;

use eframe::egui::{self, Align, Id, ScrollArea, Window};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot, PlotPoints};
use futures::channel::oneshot;
use meval::Expr;
use ode_solvers::{Dopri5, SVector, System};
use serde::{Deserialize, Serialize};


#[cfg(not(target_arch = "wasm32"))]
use rfd::FileDialog;
#[cfg(target_arch = "wasm32")]
use rfd::AsyncFileDialog;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::HtmlCanvasElement;

#[derive(Clone, Serialize, Deserialize)]
struct Row {
    /// End of Period
    end: String,
    expr: String,
}

impl Default for Row {
    fn default() -> Self {
        Self { end: "".into(), expr: "".into() }
    }
}

#[derive(Copy, Clone)]
struct DcfData {
    cashflow: f64,
    dcf_unit: f64,
    dcf_sum: f64,
}

#[derive(Clone, Serialize, Deserialize)]
struct StateData {
    rows: Vec<Row>,
    growth: String,
    discount: String,
    ode_step_size: String,
    use_log_scale: bool,
}

impl Default for StateData {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            growth: "1.02".into(),
            discount: "1.03".into(),
            ode_step_size: "0.01".into(),
            use_log_scale: false
        }
    }
}


#[derive(Default)]
pub struct AppState {
    state: StateData,

    popup_state: bool,
    popup_title: String,
    popup_msg: String,

    pending_popup: Option<oneshot::Receiver<(String, String)>>,
    pending_state: Option<oneshot::Receiver<StateData>>,

    cache: Option<(Vec<f64>, Vec<DcfData>)>,
}

impl AppState {
    fn push_row(&mut self) {
        self.state.rows.push(Row { end: "".into(), expr: "".into() });
    }

    fn pop_row(&mut self) {
        self.state.rows.pop();
    }

    fn save_file(&mut self) {

        let state = serde_json::to_string(&self.state).unwrap();

        let (tx, rx) = oneshot::channel::<(String, String)>();
        self.pending_popup = Some(rx);

        #[cfg(not(target_arch = "wasm32"))] {
            if let Some(path) = FileDialog::new()
                .add_filter("json", &["json"])
                .save_file()
            {
                let _ = match std::fs::write(path, state) {
                    Ok(_) => tx.send(("Successfully Saved".into(), "Successfully saved without any error".into())),
                    Err(e) => tx.send(("Error Occurred".into(), format!("Error while saving: {e}"))),
                };
            }
        }

        #[cfg(target_arch = "wasm32")] {
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(handle) = AsyncFileDialog::new()
                    .set_file_name("state.json")
                    .save_file()
                    .await
                {
                    let _ = match handle.write(state.as_bytes()).await {
                        Ok(_) => tx.send(("Successfully Saved".into(), "Successfully saved without any error".into())),
                        Err(e) => tx.send(("Error Occurred".into(), format!("Error while saving: {e}"))),
                    };
                }
            });
        }
    }

    fn load_file(&mut self) {

        let (tx_popup, rx_popup) = oneshot::channel::<(String, String)>();
        self.pending_popup = Some(rx_popup);

        let (tx_state, rx_state) = oneshot::channel::<StateData>();
        self.pending_state = Some(rx_state);


        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = FileDialog::new()
            .add_filter("json", &["json"])
            .pick_file()
        {
            let _ = match std::fs::read(path) {
                Ok(v) => match serde_json::from_slice::<StateData>(&v) {
                    Ok(state) => {
                        let _ = tx_state.send(state);
                        tx_popup.send(("Successfully Loaded".into(), "Successfully loaded without any error".into()))
                    },
                    Err(e) => tx_popup.send(("Error Occurred".into(), format!("Error while loading: {e}"))),
                },
                Err(e) => tx_popup.send(("Error Occurred".into(), format!("Error while loading: {e}"))),
            };
        }

        #[cfg(target_arch = "wasm32")] {
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(handle) = AsyncFileDialog::new()
                    .add_filter("json", &["json"])
                    .pick_file()
                    .await
                {
                    let _ = match serde_json::from_slice::<StateData>(&handle.read().await) {
                        Ok(state) => {
                            let _ = tx_state.send(state);
                            tx_popup.send(("Successfully Loaded".into(), "Successfully loaded without any error".into()))
                        },
                        Err(e) => tx_popup.send(("Error Occurred".into(), format!("Error while loading: {e}"))),
                    };
                }
            });
        }
        
    }

    fn calculate_cashflow(&self) -> Option<Vec<f64>> {
        let mut output: Vec<f64> = Vec::new();
        let mut prev_period: usize = 0;

        for e in self.state.rows.iter() {
            let period = e.end.parse::<usize>().unwrap_or(0);
            if period < prev_period {
                return None;
            }

            // This part is for ODE function model
            if e.expr.contains('y') {
                let rhs = match Expr::from_str(&e.expr) {
                    Ok(t) => match t.bind2("t", "y") {
                        Ok(f) => f,
                        Err(_) => {
                            output.extend(std::iter::repeat(0.0).take(period - prev_period));
                            prev_period = period;
                            continue;
                        },
                    },
                    Err(_) => {
                        output.extend(std::iter::repeat(0.0).take(period - prev_period));
                        prev_period = period;
                        continue;
                    },
                };

                struct Sys { f: Box<dyn Fn(f64, f64)->f64> }
                impl System<f64, SVector<f64, 1>> for Sys {
                    fn system(&self, t: f64, y: &SVector<f64, 1>, dy: &mut SVector<f64, 1>) {
                        dy[0] = (self.f)(t, y[0]);
                    }
                }

                let mut solver = Dopri5::new(
                    Sys{f: Box::new(rhs)}, // Right-Hand Side
                    0.0, (period - prev_period) as f64, self.state.ode_step_size.parse().unwrap_or(1.0), // t0, t_end, h
                    [output.last().cloned().unwrap_or(0.0) as f64].into(),          // Initial Value: y(0)
                    1e-10, 1e-10           // Error limit
                );
                match solver.integrate() {
                    Ok(_) => {
                        let x_out = solver.x_out();
                        let y_out: Vec<f64> = solver.y_out().iter().map(|v| v[0]).collect();
                        let step = x_out[1] - x_out[0];
                        let mut n_counter: usize = output.is_empty().then(|| 0).unwrap_or(1);
                        for (i, &x) in x_out.iter().enumerate() {
                            if x - (n_counter as f64) > -step {
                                output.push(y_out[i]);
                                n_counter += 1;
                            }
                        }
                        prev_period = period;
                    },
                    Err(_) => {
                        output.extend(std::iter::repeat(0.0).take(period - prev_period));
                        prev_period = period;
                        continue;
                    },
                }

            // This part is just for univariant function model
            } else if e.expr.contains('t') {
                let expr = match Expr::from_str(&e.expr) {
                    Ok(t) => t,
                    Err(_) => {
                        output.extend(std::iter::repeat(0.0).take(period - prev_period));
                        prev_period = period;
                        continue;
                    },
                };

                let f = match expr.bind("t") {
                    Ok(t) => t,
                    Err(_) => {
                        output.extend(std::iter::repeat(0.0).take(period - prev_period));
                        prev_period = period;
                        continue;
                    },
                };

                if output.is_empty() {
                    output.push(f(0.0));
                }

                for t in 1..=(period - prev_period) {
                    output.push(f(t as f64));
                }

                prev_period = period;
                
            // This part is for constant function model
            } else {
                let expr = match Expr::from_str(&e.expr) {
                    Ok(t) => t,
                    Err(_) => {
                        output.extend(std::iter::repeat(0.0).take(period - prev_period));
                        prev_period = period;
                        continue;
                    },
                };

                let constant = match expr.eval() {
                    Ok(t) => t,
                    Err(_) => {
                        output.extend(std::iter::repeat(0.0).take(period - prev_period));
                        prev_period = period;
                        continue;
                    },
                };

                if output.is_empty() {
                    output.push(constant);
                }

                for _ in 1..=(period - prev_period) {
                    output.push(constant);
                }

                prev_period = period;

            }
        }

        Some(output)
    }

    fn calculate_dcf(&self, cashflow: &[f64]) -> Vec<DcfData> {
        let mut output = Vec::new();
        let mut discount = 1.0;
        let mut dcf_sum = 0.0;
        for &cashflow in cashflow.iter() {
            let dcf_unit = cashflow / discount;
            dcf_sum += dcf_unit;
            discount *= self.state.discount.parse::<f64>().unwrap_or(1.0);
            output.push(DcfData { cashflow, dcf_unit, dcf_sum });
        }
        output
    }

    fn show_popup(&mut self, title: String, msg: String) {
        self.popup_state = true;
        self.popup_title = title;
        self.popup_msg = msg;
    }

    fn close_popup(&mut self) {
        self.popup_state = false;
    }
}

/* ───────── egui App implementation ───────── */
impl eframe::App for AppState {

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {

        let retain_float = |c: char, dots: &mut usize| -> bool {
            if c == '.' {
                *dots += 1;
                *dots <= 1
            } else {
                c.is_ascii_digit()
            }
        };

        if let Some(rx) = &mut self.pending_popup {
            match rx.try_recv() {
                Ok(Some((title, msg))) => {
                    self.show_popup(title, msg);
                },
                Err(e) => {
                    log::error!("Error while loading popup: {e}");
                },
                _ => {},
            }
            self.pending_popup = None;
        }

        if let Some(rx) = &mut self.pending_state {
            match rx.try_recv() {
                Ok(Some(state)) => {
                    self.state = state;
                },
                Err(e) => {
                    log::error!("Error while loading state: {e}");
                },
                _ => {},
            }
            self.pending_state = None;
            self.cache = None;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::A)) {
            self.push_row();
        };

        if ctx.input(|i| i.key_pressed(egui::Key::D)) {
            self.pop_row();
        };

        if ctx.input(|i: &egui::InputState| i.key_pressed(egui::Key::S)) {
            self.save_file();
        };

        if ctx.input(|i| i.key_pressed(egui::Key::L)) {
            self.load_file();
        };
        

        egui::SidePanel::left(Id::new("leftside")).show(ctx, |ui| {
            // 1) Control Buttons
            ui.horizontal(|ui| {
                if ui.button("Add").clicked() {
                    self.push_row();
                }
                if ui.button("Delete").clicked() {
                    self.pop_row();
                }
                if ui.button("Save").clicked() {
                    self.save_file();
                }
                if ui.button("Load").clicked() {
                    self.load_file();
                }
            });
            
            ui.separator();


            // 2) Draw Rows
            let grid = egui::Grid::new("ranges_grid")
                .spacing([8.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    let mut prev_start = String::from("0");   // first row means start value

                    for row in &mut self.state.rows {
                        ui.label(&prev_start);

                        ui.label(" ~ ");

                        if ui.add(
                            egui::TextEdit::singleline(&mut row.end)
                                .desired_width(80.0)
                                .hint_text("End"),
                        ).changed() {
                            self.cache = None;

                            row.end.retain(|c| c.is_ascii_digit());
                        }

                        if ui.add(
                            egui::TextEdit::singleline(&mut row.expr)
                                .hint_text("Expression")
                        ).changed() {
                            self.cache = None;
                        }

                        ui.end_row();

                        prev_start = row.end.clone();
                    }

                    ui.label(&prev_start);
                    ui.label(" ~ ");
                    ui.label("∞");
                    ui.horizontal(|ui| {
                        if ui.add(
                            egui::TextEdit::singleline(&mut self.state.growth)
                                .desired_width(60.0)
                                .hint_text("Growth"),
                        ).changed() {
                            self.cache = None;

                            let mut dot_counter: usize = 0;
                            self.state.growth.retain(|c| retain_float(c, &mut dot_counter));
                        }
                        ui.add(egui::Label::new(format!(" ^ t * y[{prev_start}]")));
                    });

                });
            
            let grid_width = grid.response.rect.right() - grid.response.rect.left();

            // 3) discount rate
            ui.horizontal(|ui| {
                ui.set_width(grid_width);
                ui.label("Discount Rate (e.g. WACC): ");
                if ui.text_edit_singleline(&mut self.state.discount).changed() {
                    self.cache = None;

                    let mut dot_counter: usize = 0;
                    self.state.discount.retain(|c| retain_float(c, &mut dot_counter));
                }
            });

            // 4) step size
            ui.horizontal(|ui| {
                ui.set_width(grid_width);
                ui.label("Step Size for ODE Solver: ");
                if ui.text_edit_singleline(&mut self.state.ode_step_size).changed() {
                    self.cache = None;

                    let mut dot_counter: usize = 0;
                    self.state.ode_step_size.retain(|c| retain_float(c, &mut dot_counter));
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {

            ui.horizontal(|ui| {
                ui.heading("Cash Flow Expectation");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ui.checkbox(&mut self.state.use_log_scale, "Log Scale");
                });
            });

            if self.cache.is_none() {
                if let Some(cashflow) = self.calculate_cashflow() {
                    let dcf_data = self.calculate_dcf(&cashflow);
                    self.cache = Some((cashflow, dcf_data));
                }
            }

            if let Some((cashflow, dcf_data)) = &self.cache {
                let points: PlotPoints  = cashflow.iter().enumerate().map(|(x, &y)| {
                    if self.state.use_log_scale {
                        [x as f64, f64::max(0.0, y.log10())]
                    } else {
                        [x as f64, y]
                    }
                }).collect();
                Plot::new("my_plot")
                    .view_aspect(2.0)
                    .show(ui, |plot_ui| {
                        plot_ui.line(Line::new("Cash Flow Expectation", points));
                    });

                ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(ui.available_height() - 20.0)
                    .show(ui, |ui| {
                        TableBuilder::new(ui)
                            .striped(true)
                            .column(Column::remainder())
                            .column(Column::remainder())
                            .column(Column::remainder())
                            .column(Column::remainder())
                            .header(22.0, |mut header| {
                                header.col(|ui| { ui.strong("t"); });
                                header.col(|ui| { ui.strong("Cashflow");  });
                                header.col(|ui| { ui.strong("UNIT DCF");  });
                                header.col(|ui| { ui.strong("Sum of DCF");  });
                            })
                            .body(|mut body| {
                                for (t, &data) in dcf_data.iter().enumerate() {
                                    body.row(16.0, |mut row| {
                                        row.col(|ui| { ui.label(t.to_string()); });
                                        row.col(|ui| { ui.label(data.cashflow.to_string()); });
                                        row.col(|ui| { ui.label(data.dcf_unit.to_string()); });
                                        row.col(|ui| { ui.label(data.dcf_sum.to_string()); });
                                    });
                                }
                            });
                    });
                
                let terminal_value = dcf_data.last().map(|d| {
                    let growth: f64 = self.state.growth.parse().unwrap_or(1.0);
                    (d.cashflow * growth) / (self.state.discount.parse::<f64>().unwrap_or(1.0) - growth)
                }).unwrap_or(0.0);

                ui.horizontal(|ui| {
                    ui.strong(format!("Terminal Value: {terminal_value}"));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        ui.heading(format!("DCF Result: {}", terminal_value + dcf_data.last().map(|d| d.dcf_sum).unwrap_or(0.0)));
                    });
                });
            }
               
        });

        if self.popup_state {
            Window::new(&self.popup_title)
                .resizable([false; 2])
                .show(ctx, |ui| {
                    ui.label(&self.popup_msg);
                    if ui.button("OK").clicked() {
                        self.close_popup();
                    }
                });
        }
    }
}


/// Your handle to the web app from JavaScript.
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
#[wasm_bindgen]
pub struct WebHandle {
    runner: eframe::WebRunner,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl WebHandle {
    /// Installs a panic hook, then returns.
    #[allow(clippy::new_without_default)]
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        // Redirect [`log`] message to `console.log` and friends:
        eframe::WebLogger::init(log::LevelFilter::Debug).ok();

        Self {
            runner: eframe::WebRunner::new(),
        }
    }

    /// Call this once from JavaScript to start your app.
    #[wasm_bindgen]
    pub async fn start(&self, canvas: HtmlCanvasElement) -> Result<(), wasm_bindgen::JsValue> {
        self.runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_| Ok(Box::new(AppState::default())),)
            )
            .await
    }

    // The following are optional:

    /// Shut down eframe and clean up resources.
    #[wasm_bindgen]
    pub fn destroy(&self) {
        self.runner.destroy();
    }

    /// The JavaScript can check whether or not your app has crashed:
    #[wasm_bindgen]
    pub fn has_panicked(&self) -> bool {
        self.runner.has_panicked()
    }

    #[wasm_bindgen]
    pub fn panic_message(&self) -> Option<String> {
        self.runner.panic_summary().map(|s| s.message())
    }

    #[wasm_bindgen]
    pub fn panic_callstack(&self) -> Option<String> {
        self.runner.panic_summary().map(|s| s.callstack())
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub async fn wasm_start() -> Result<(), wasm_bindgen::JsValue> {

    let canvas: HtmlCanvasElement = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("egui_canvas"))
        .expect("canvas tag with id='egui_canvas' not found")
        .dyn_into::<HtmlCanvasElement>()
        .expect("element is not a canvas");

    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    eframe::WebRunner::new().start(
        canvas,
        eframe::WebOptions::default(),
        Box::new(|_| Ok(Box::<AppState>::default())),
    )
    .await
}