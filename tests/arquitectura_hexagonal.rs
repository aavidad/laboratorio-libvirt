// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

const DOMINIO: &[(&str, &str)] = &[
    (
        "identificador.rs",
        include_str!("../src/dominio/identificador.rs"),
    ),
    ("plantilla.rs", include_str!("../src/dominio/plantilla.rs")),
    ("reserva.rs", include_str!("../src/dominio/reserva.rs")),
];

const APLICACION: &[(&str, &str)] = &[
    (
        "gestionar_reserva.rs",
        include_str!("../src/aplicacion/gestionar_reserva.rs"),
    ),
    ("ordenes.rs", include_str!("../src/aplicacion/ordenes.rs")),
    ("puertos.rs", include_str!("../src/aplicacion/puertos.rs")),
];

#[test]
fn el_dominio_no_conoce_capas_externas() {
    for (nombre, contenido) in DOMINIO {
        for prohibido in [
            "crate::adaptadores",
            "crate::aplicacion",
            "axum::",
            "tokio::",
            "std::fs",
            "std::process",
        ] {
            assert!(
                !contenido.contains(prohibido),
                "{nombre} depende de {prohibido}"
            );
        }
    }
}

#[test]
fn los_casos_de_uso_solo_dependen_del_dominio_y_sus_puertos() {
    for (nombre, contenido) in APLICACION {
        for prohibido in [
            "crate::adaptadores",
            "axum::",
            "tokio::",
            "xmltree::",
            "std::fs",
            "std::process",
        ] {
            assert!(
                !contenido.contains(prohibido),
                "{nombre} depende de {prohibido}"
            );
        }
    }
}
