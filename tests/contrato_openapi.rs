// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use laboratorio_libvirt::dominio::acceso::{diagnosticar_acceso, EstadoAccesoObservado};
use laboratorio_libvirt::dominio::identificador::Identificador;
use serde_yaml_ng::Value;

#[test]
fn el_contrato_openapi_incluye_los_flujos_tipados_y_resuelve_sus_referencias() {
    let documento: Value = serde_yaml_ng::from_str(include_str!("../docs/api-v1.yaml")).unwrap();
    assert_eq!(
        documento["openapi"].as_str(),
        Some("3.1.0"),
        "la versión OpenAPI debe ser explícita"
    );
    let paths = documento["paths"].as_mapping().unwrap();
    for ruta in [
        "/api/v1/destinos-promocion",
        "/api/v1/reservas/{id}/diagnostico-acceso",
        "/api/v1/reservas/{id}/diagnostico-arranque",
        "/api/v1/reservas/{id}/preparacion/sanear",
        "/api/v1/reservas/{id}/preparacion/iniciar-ciclo",
        "/api/v1/reservas/{id}/preparacion/detener-ciclo",
        "/api/v1/reservas/{id}/preparacion/validar-ciclo",
        "/api/v1/reservas/{id}/preparacion/promover",
    ] {
        assert!(
            paths.contains_key(Value::String(ruta.to_owned())),
            "falta {ruta}"
        );
    }
    let identificador = &documento["components"]["schemas"]["Identificador"];
    assert_eq!(identificador["minLength"].as_u64(), Some(3));
    assert_eq!(identificador["maxLength"].as_u64(), Some(64));
    assert_eq!(
        identificador["pattern"].as_str(),
        Some("^[a-z0-9][a-z0-9-]{2,63}$")
    );
    for valor in ["abc", "0-1", "identificador-terminado-"] {
        assert!(Identificador::nuevo(valor).is_ok());
    }
    for valor in ["ab", "Mayuscula", "guion_bajo", "ámbito"] {
        assert!(Identificador::nuevo(valor).is_err());
    }
    let codigos = documento["components"]["schemas"]["CodigoComprobacionAcceso"]["enum"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap();
    assert_eq!(
        codigos,
        vec![
            "reserva_en_ejecucion",
            "instancia_encendida",
            "direccion_observable",
            "direccion_lease_observable",
            "direccion_agente_observable",
            "direccion_arp_observable",
            "qga_disponible",
            "marca_uuid_presente",
            "marca_uuid_coincidente",
            "clave_servidor_ssh_presente",
            "clave_servidor_ssh_valida",
        ]
    );
    let diagnostico = diagnosticar_acceso(
        true,
        true,
        EstadoAccesoObservado {
            instancia_encendida: true,
            direccion_lease_observable: true,
            direccion_agente_observable: true,
            direccion_arp_observable: true,
            qga_disponible: true,
            marca_uuid_presente: true,
            marca_uuid_coincidente: true,
            clave_servidor_ssh_presente: true,
            clave_servidor_ssh_valida: true,
        },
    );
    let diagnostico_json = serde_json::to_value(diagnostico).unwrap();
    let codigos_rust = diagnostico_json["comprobaciones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|comprobacion| comprobacion["codigo"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(codigos, codigos_rust);
    let comprobaciones =
        &documento["components"]["schemas"]["DiagnosticoAcceso"]["properties"]["comprobaciones"];
    assert_eq!(comprobaciones["minItems"].as_u64(), Some(11));
    assert_eq!(comprobaciones["maxItems"].as_u64(), Some(11));
    let comprobaciones_arranque =
        &documento["components"]["schemas"]["DiagnosticoArranque"]["properties"]["comprobaciones"];
    assert_eq!(comprobaciones_arranque["minItems"].as_u64(), Some(8));
    assert_eq!(comprobaciones_arranque["maxItems"].as_u64(), Some(8));
    assert!(
        documento["components"]["schemas"]["CodigoComprobacionArranque"]["enum"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|codigo| codigo.as_str() == Some("recursos_escritura_disponibles"))
    );
    let validacion_ciclo = &documento["components"]["schemas"]["ValidacionCicloPlantilla"];
    assert!(validacion_ciclo["required"]
        .as_sequence()
        .unwrap()
        .iter()
        .any(|campo| campo.as_str() == Some("identidad_ssh")));
    assert_eq!(
        validacion_ciclo["properties"]["identidad_ssh"]["$ref"].as_str(),
        Some("#/components/schemas/ComprobacionIdentidadSsh")
    );
    let progreso = &documento["components"]["schemas"]["ProgresoPreparacionPlantilla"];
    assert!(progreso["required"]
        .as_sequence()
        .unwrap()
        .iter()
        .any(|campo| campo.as_str() == Some("identidad_ssh_en_curso")));
    let identidad_servidor = &documento["components"]["schemas"]["IdentidadServidor"];
    assert_eq!(
        identidad_servidor["properties"]["algoritmo"]["const"].as_str(),
        Some("ssh-ed25519")
    );
    assert_eq!(
        identidad_servidor["properties"]["clave_publica"]["pattern"].as_str(),
        Some("^ssh-ed25519 [A-Za-z0-9+/]{68}$")
    );
    comprobar_referencias(&documento, &documento);
}

fn comprobar_referencias(raiz: &Value, valor: &Value) {
    match valor {
        Value::Mapping(mapa) => {
            if let Some(referencia) = mapa
                .get(Value::String("$ref".to_owned()))
                .and_then(Value::as_str)
            {
                let puntero = referencia
                    .strip_prefix('#')
                    .expect("solo se admiten referencias locales");
                assert!(
                    resolver_puntero(raiz, puntero).is_some(),
                    "referencia OpenAPI no resuelta: {referencia}"
                );
            }
            for elemento in mapa.values() {
                comprobar_referencias(raiz, elemento);
            }
        }
        Value::Sequence(secuencia) => {
            for elemento in secuencia {
                comprobar_referencias(raiz, elemento);
            }
        }
        _ => {}
    }
}

fn resolver_puntero<'a>(raiz: &'a Value, puntero: &str) -> Option<&'a Value> {
    let mut actual = raiz;
    for componente in puntero.strip_prefix('/')?.split('/') {
        let clave = componente.replace("~1", "/").replace("~0", "~");
        actual = actual.as_mapping()?.get(Value::String(clave))?;
    }
    Some(actual)
}
