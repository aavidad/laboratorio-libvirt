// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fábrica hexagonal de máquinas virtuales efímeras Windows y Linux. Los
//! consumidores se integran por CLI o API, no mediante dependencias del núcleo.

pub mod adaptadores;
pub mod aplicacion;
pub mod composicion;
pub mod dominio;
pub mod presentacion;
