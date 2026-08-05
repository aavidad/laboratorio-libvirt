// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::adaptadores::configuracion_json::ConfiguracionLocal;
use crate::aplicacion::ordenes::Orden;
use crate::composicion;
use crate::dominio::identificador::Identificador;
use crate::dominio::reserva::MotivoFallo;
use crate::presentacion::i18n::{CatalogoMensajes, Traductor};
use anyhow::{bail, Context, Result};
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path as Ruta, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const MAXIMO_BYTES_TOKEN: u64 = 1024;
const MAXIMO_BYTES_SOLICITUD: usize = 64 * 1024;

#[derive(Clone)]
struct EstadoApi {
    configuracion: ConfiguracionLocal,
    resumen_token: [u8; 32],
    idioma_predeterminado: String,
    directorio_idiomas: Option<std::path::PathBuf>,
}

type EstadoCompartido = Arc<EstadoApi>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolicitudPreparar {
    id_plantilla: Identificador,
    id_ejecucion: Identificador,
    confirmacion: Identificador,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolicitudConfirmacion {
    confirmacion: Identificador,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolicitudSanearPlantilla {
    id_destino: Identificador,
    confirmacion: Identificador,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolicitudFallo {
    motivo: MotivoFallo,
    confirmacion: Identificador,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolicitudDescarteFallido {
    acepta_perdida_resultados: bool,
    confirmacion: Identificador,
}

#[derive(Debug, Serialize)]
struct Salud {
    estado: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorApi {
    codigo: &'static str,
    mensaje: String,
}

pub async fn servir(configuracion: ConfiguracionLocal) -> Result<()> {
    let configuracion_api = configuracion
        .api
        .clone()
        .context("la configuración no habilita la API")?;
    let token = cargar_token(&configuracion_api.fichero_token)?;
    let estado = Arc::new(EstadoApi {
        idioma_predeterminado: configuracion.idioma_predeterminado.clone(),
        directorio_idiomas: configuracion.directorio_idiomas.clone(),
        configuracion,
        resumen_token: Sha256::digest(&token).into(),
    });
    let rutas = construir_rutas(estado);
    let escucha = tokio::net::TcpListener::bind(configuracion_api.escucha)
        .await
        .context("no se pudo abrir la escucha local de la API")?;
    axum::serve(escucha, rutas)
        .with_graceful_shutdown(esperar_cancelacion())
        .await
        .context("la API terminó con error")
}

fn construir_rutas(estado: EstadoCompartido) -> Router {
    Router::new()
        .route("/api/v1/salud", get(salud))
        .route("/api/v1/plantillas", get(listar_plantillas))
        .route("/api/v1/destinos-promocion", get(listar_destinos_promocion))
        .route(
            "/api/v1/plantillas/{id}/diagnostico",
            get(inspeccionar_plantilla),
        )
        .route("/api/v1/reservas", get(listar_reservas).post(preparar))
        .route("/api/v1/reservas/{id}", get(estado_reserva))
        .route("/api/v1/reservas/{id}/acceso", get(acceso_reserva))
        .route(
            "/api/v1/reservas/{id}/diagnostico-acceso",
            get(diagnostico_acceso_reserva),
        )
        .route(
            "/api/v1/reservas/{id}/diagnostico-arranque",
            get(diagnostico_arranque_reserva),
        )
        .route("/api/v1/reservas/{id}/iniciar", post(iniciar))
        .route("/api/v1/reservas/{id}/detener", post(detener))
        .route("/api/v1/reservas/{id}/reconciliar", post(reconciliar))
        .route(
            "/api/v1/reservas/{id}/proteger-resultados",
            post(proteger_resultados),
        )
        .route("/api/v1/reservas/{id}/fallar", post(fallar))
        .route("/api/v1/reservas/{id}/descartar", post(descartar))
        .route(
            "/api/v1/reservas/{id}/descartar-fallida",
            post(descartar_fallida),
        )
        .route(
            "/api/v1/reservas/{id}/preparacion/sanear",
            post(sanear_plantilla),
        )
        .route(
            "/api/v1/reservas/{id}/preparacion/iniciar-ciclo",
            post(iniciar_ciclo_plantilla),
        )
        .route(
            "/api/v1/reservas/{id}/preparacion/detener-ciclo",
            post(detener_ciclo_plantilla),
        )
        .route(
            "/api/v1/reservas/{id}/preparacion/validar-ciclo",
            post(validar_ciclo_plantilla),
        )
        .route(
            "/api/v1/reservas/{id}/preparacion/promover",
            post(promover_plantilla),
        )
        .layer(DefaultBodyLimit::max(MAXIMO_BYTES_SOLICITUD))
        .route_layer(middleware::from_fn_with_state(estado.clone(), autenticar))
        .with_state(estado)
}

async fn esperar_cancelacion() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn autenticar(
    State(estado): State<EstadoCompartido>,
    solicitud: Request<Body>,
    siguiente: Next,
) -> Response {
    let autorizado = solicitud
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|valor| valor.as_bytes().strip_prefix(b"Bearer "))
        .filter(|valor| (32..=512).contains(&valor.len()))
        .is_some_and(|valor| comparacion_constante(&Sha256::digest(valor), &estado.resumen_token));
    if autorizado {
        siguiente.run(solicitud).await
    } else {
        respuesta_error(
            StatusCode::UNAUTHORIZED,
            "no_autorizado",
            "error_no_autorizado",
            solicitud.headers(),
            &estado,
        )
    }
}

async fn salud() -> Json<Salud> {
    Json(Salud {
        estado: "disponible",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn listar_plantillas(
    State(estado): State<EstadoCompartido>,
    cabeceras: HeaderMap,
) -> Response {
    ejecutar_orden(estado, cabeceras, Orden::ListarPlantillas, StatusCode::OK).await
}

async fn listar_reservas(State(estado): State<EstadoCompartido>, cabeceras: HeaderMap) -> Response {
    ejecutar_orden(estado, cabeceras, Orden::ListarReservas, StatusCode::OK).await
}

async fn listar_destinos_promocion(
    State(estado): State<EstadoCompartido>,
    cabeceras: HeaderMap,
) -> Response {
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::ListarDestinosPromocion,
        StatusCode::OK,
    )
    .await
}

async fn inspeccionar_plantilla(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
) -> Response {
    let Ok(id_plantilla) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::Inspeccionar { id_plantilla },
        StatusCode::OK,
    )
    .await
}

async fn estado_reserva(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
) -> Response {
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::Estado { id_ejecucion },
        StatusCode::OK,
    )
    .await
}

async fn acceso_reserva(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
) -> Response {
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::Acceso { id_ejecucion },
        StatusCode::OK,
    )
    .await
}

async fn diagnostico_acceso_reserva(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
) -> Response {
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::DiagnosticarAcceso { id_ejecucion },
        StatusCode::OK,
    )
    .await
}

async fn diagnostico_arranque_reserva(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
) -> Response {
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::DiagnosticarArranque { id_ejecucion },
        StatusCode::OK,
    )
    .await
}

async fn preparar(
    State(estado): State<EstadoCompartido>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudPreparar>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::Preparar {
            id_plantilla: solicitud.id_plantilla,
            id_ejecucion: solicitud.id_ejecucion,
            confirmacion: solicitud.confirmacion,
        },
        StatusCode::CREATED,
    )
    .await
}

async fn iniciar(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::Iniciar {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn detener(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::Detener {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn reconciliar(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::Reconciliar {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn proteger_resultados(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::ProtegerResultados {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn descartar(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::Descartar {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn fallar(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudFallo>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::MarcarFallida {
            id_ejecucion,
            motivo: solicitud.motivo,
            confirmacion: solicitud.confirmacion,
        },
        StatusCode::OK,
    )
    .await
}

async fn descartar_fallida(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudDescarteFallido>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::DescartarFallida {
            id_ejecucion,
            confirmacion: solicitud.confirmacion,
            acepta_perdida_resultados: solicitud.acepta_perdida_resultados,
        },
        StatusCode::OK,
    )
    .await
}

async fn sanear_plantilla(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudSanearPlantilla>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        Orden::SanearPlantilla {
            id_ejecucion,
            id_destino: solicitud.id_destino,
            confirmacion: solicitud.confirmacion,
        },
        StatusCode::OK,
    )
    .await
}

async fn iniciar_ciclo_plantilla(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::IniciarCicloPlantilla {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn detener_ciclo_plantilla(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::DetenerCicloPlantilla {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn validar_ciclo_plantilla(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::ValidarCicloPlantilla {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn promover_plantilla(
    State(estado): State<EstadoCompartido>,
    Ruta(id): Ruta<String>,
    cabeceras: HeaderMap,
    cuerpo: std::result::Result<Json<SolicitudConfirmacion>, JsonRejection>,
) -> Response {
    let Ok(Json(solicitud)) = cuerpo else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    mutacion_confirmada(
        estado,
        id,
        cabeceras,
        solicitud,
        |id_ejecucion, confirmacion| Orden::PromoverPlantilla {
            id_ejecucion,
            confirmacion,
        },
    )
    .await
}

async fn mutacion_confirmada<F>(
    estado: EstadoCompartido,
    id: String,
    cabeceras: HeaderMap,
    solicitud: SolicitudConfirmacion,
    construir: F,
) -> Response
where
    F: FnOnce(Identificador, Identificador) -> Orden,
{
    let Ok(id_ejecucion) = Identificador::nuevo(id) else {
        return solicitud_no_valida(&cabeceras, &estado);
    };
    ejecutar_orden(
        estado,
        cabeceras,
        construir(id_ejecucion, solicitud.confirmacion),
        StatusCode::OK,
    )
    .await
}

async fn ejecutar_orden(
    estado: EstadoCompartido,
    cabeceras: HeaderMap,
    orden: Orden,
    codigo_exito: StatusCode,
) -> Response {
    let configuracion = estado.configuracion.clone();
    match tokio::task::spawn_blocking(move || composicion::ejecutar(configuracion, orden)).await {
        Ok(Ok(respuesta)) => (codigo_exito, Json(respuesta)).into_response(),
        Ok(Err(_)) => respuesta_error(
            StatusCode::CONFLICT,
            "operacion_rechazada",
            "error_interno",
            &cabeceras,
            &estado,
        ),
        Err(_) => respuesta_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "tarea_interna_fallida",
            "error_interno",
            &cabeceras,
            &estado,
        ),
    }
}

fn solicitud_no_valida(cabeceras: &HeaderMap, estado: &EstadoCompartido) -> Response {
    respuesta_error(
        StatusCode::BAD_REQUEST,
        "solicitud_no_valida",
        "error_solicitud_no_valida",
        cabeceras,
        estado,
    )
}

fn respuesta_error(
    estado_http: StatusCode,
    codigo: &'static str,
    clave: &str,
    cabeceras: &HeaderMap,
    estado: &EstadoCompartido,
) -> Response {
    let solicitado = cabeceras
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|valor| valor.to_str().ok())
        .unwrap_or(&estado.idioma_predeterminado);
    let catalogo = CatalogoMensajes::seleccionar_con_directorio(
        Some(solicitado),
        estado.directorio_idiomas.as_deref(),
    )
    .or_else(|_| CatalogoMensajes::seleccionar(Some("es")))
    .expect("el catálogo castellano integrado se valida en las pruebas");
    (
        estado_http,
        Json(ErrorApi {
            codigo,
            mensaje: catalogo.texto(clave).to_owned(),
        }),
    )
        .into_response()
}

fn cargar_token(ruta: &std::path::Path) -> Result<Vec<u8>> {
    let metadatos = fs::symlink_metadata(ruta)
        .with_context(|| format!("no se pudo inspeccionar {}", ruta.display()))?;
    if metadatos.file_type().is_symlink() || !metadatos.is_file() {
        bail!("el token de la API debe ser un fichero ordinario");
    }
    if !(32..=MAXIMO_BYTES_TOKEN).contains(&metadatos.len()) {
        bail!("el token de la API tiene una longitud no permitida");
    }
    #[cfg(unix)]
    {
        if metadatos.mode() & 0o077 != 0 {
            bail!("el token de la API permite acceso a grupo u otros usuarios");
        }
        // SAFETY: geteuid no recibe punteros ni tiene precondiciones.
        if metadatos.uid() != unsafe { libc::geteuid() } {
            bail!("el token de la API no pertenece al usuario actual");
        }
    }
    let mut token = Vec::with_capacity(metadatos.len() as usize);
    File::open(ruta)?.read_to_end(&mut token)?;
    while token
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        token.pop();
    }
    if !(32..=512).contains(&token.len())
        || !token
            .iter()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
    {
        bail!("el token de la API no cumple el formato requerido");
    }
    Ok(token)
}

fn comparacion_constante(izquierda: &[u8], derecha: &[u8]) -> bool {
    if izquierda.len() != derecha.len() {
        return false;
    }
    izquierda
        .iter()
        .zip(derecha)
        .fold(0_u8, |acumulado, (a, b)| acumulado | (a ^ b))
        == 0
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use http_body_util::BodyExt;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    #[test]
    fn compara_resumenes_sin_atajo_por_contenido() {
        assert!(comparacion_constante(b"abc", b"abc"));
        assert!(!comparacion_constante(b"abc", b"abd"));
        assert!(!comparacion_constante(b"abc", b"ab"));
    }

    #[test]
    fn carga_un_token_privado_sin_conservar_el_salto_final() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("token");
        File::create(&ruta)
            .unwrap()
            .write_all(b"test-token-not-a-secret-00000000\n")
            .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(cargar_token(&ruta).unwrap().len(), 32);
    }

    fn api_de_prueba() -> (tempfile::TempDir, Router, &'static str) {
        let temporal = tempfile::tempdir().unwrap();
        let raiz_recibos = temporal.path().join("privado/recibos");
        let raiz_resultados = temporal.path().join("privado/resultados");
        fs::create_dir_all(&raiz_recibos).unwrap();
        fs::create_dir_all(&raiz_resultados).unwrap();
        #[cfg(unix)]
        {
            fs::set_permissions(&raiz_recibos, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&raiz_resultados, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let token = "test-token-not-a-secret-00000000";
        let ruta_token = temporal.path().join("privado/token");
        File::create(&ruta_token)
            .unwrap()
            .write_all(token.as_bytes())
            .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta_token, fs::Permissions::from_mode(0o600)).unwrap();
        let ruta_configuracion = temporal.path().join("configuracion.json");
        serde_json::to_writer(
            File::create(&ruta_configuracion).unwrap(),
            &serde_json::json!({
                "version": 2,
                "uri_libvirt": "qemu:///system",
                "raiz_recibos": raiz_recibos,
                "raiz_resultados": raiz_resultados,
                "api": {
                    "escucha": "127.0.0.1:0",
                    "fichero_token": ruta_token
                },
                "plantillas": [{
                    "id": "linux-pruebas",
                    "sistema": "linux",
                    "politica_red": "sin_red",
                    "dominio": "linux-base",
                    "uuid_esperado": "11111111-1111-1111-1111-111111111111",
                    "destino_disco": "vda",
                    "pool_plantilla": "plantillas",
                    "volumen_plantilla": "linux-base.qcow2",
                    "pool_instancias": "instancias"
                }]
            }),
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta_configuracion, fs::Permissions::from_mode(0o600)).unwrap();
        let configuracion = ConfiguracionLocal::cargar(&ruta_configuracion).unwrap();
        let estado = Arc::new(EstadoApi {
            configuracion,
            resumen_token: Sha256::digest(token.as_bytes()).into(),
            idioma_predeterminado: "es".to_owned(),
            directorio_idiomas: None,
        });
        (temporal, construir_rutas(estado), token)
    }

    #[tokio::test]
    async fn la_api_exige_autenticacion_incluso_para_salud() {
        let (_temporal, api, _) = api_de_prueba();
        let respuesta = api
            .oneshot(
                Request::builder()
                    .uri("/api/v1/salud")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(respuesta.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn la_api_acepta_un_token_valido() {
        let (_temporal, api, token) = api_de_prueba();
        let respuesta = api
            .oneshot(
                Request::builder()
                    .uri("/api/v1/salud")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(respuesta.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn la_api_redacta_en_castellano_un_json_no_valido() {
        let (_temporal, api, token) = api_de_prueba();
        let respuesta = api
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/reservas")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(respuesta.status(), StatusCode::BAD_REQUEST);
        let cuerpo = respuesta.into_body().collect().await.unwrap().to_bytes();
        let texto = String::from_utf8(cuerpo.to_vec()).unwrap();
        assert!(texto.contains("solicitud no válida"));
        assert!(!texto.contains("expected"));
    }
}
