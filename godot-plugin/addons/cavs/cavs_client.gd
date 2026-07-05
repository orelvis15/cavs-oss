## Cliente CAVS para Godot 4 — GDScript puro, sin dependencias nativas.
##
## Habla el protocolo CAVS completo contra un cavs-server:
##   1. GET  /api/assets/{asset}/manifest            (JSON)
##   2. POST /api/assets/{asset}/sessions            (have-set de la caché local)
##   3. POST /api/sessions/{id}/batch                (CVSP v2 binario: refs + chunks)
##
## Mantiene una caché content-addressable persistente en user://, resuelve
## las referencias localmente (solo descarga chunks que no tiene), descomprime
## el wire con el zstd nativo de Godot, reconstruye los archivos originales y
## verifica su SHA-256 contra el manifiesto firmado por el packer.
##
## Uso típico (pantalla de carga):
##   var cavs := CavsClient.new("https://cdn.mijuego.com")
##   var result := cavs.fetch("game_v2")          # bloqueante: usar Thread
##   if result.ok:
##       ProjectSettings.load_resource_pack(result.files[0])
##
## O en una línea: CavsClient.new(url).ensure_pack("game_v2")
class_name CavsClient
extends RefCounted

## Progreso de descarga/reconstrucción para barras de carga.
## `done`/`total` son bytes lógicos (los refs de caché también avanzan).
## Se emite desde el hilo de fetch: conecta con CONNECT_DEFERRED si tocas UI
## (fetch_async ya entrega su resultado en el hilo principal).
signal progress(done: int, total: int, stage: String)

const BATCH_SEGMENTS := 64
const WIRE_ZSTD := 1

var base_url: String
var cache_dir: String = "user://cavs_cache"
## Ruta a un certificado PEM confiable (para TLS self-signed en desarrollo).
var ca_cert_path: String = ""
## Si el manifiesto trae sha256 por archivo, exigir que verifique.
var require_sha256: bool = true
## Reintentos por request HTTP ante errores de red o 5xx.
var max_retries: int = 3
## Backoff exponencial: base * 2^intento (ms).
var retry_base_ms: int = 250
## Timeout por request (conexión + respuesta), en ms.
var request_timeout_ms: int = 30_000

var _scheme: String
var _host: String
var _port: int
var _threads: Array[Thread] = []


func _init(url: String = "http://127.0.0.1:8990") -> void:
	base_url = url
	var regex := RegEx.new()
	regex.compile("^(https?)://([^:/]+)(?::(\\d+))?")
	var m := regex.search(url)
	assert(m != null, "URL inválida: " + url)
	_scheme = m.get_string(1)
	_host = m.get_string(2)
	var port_str := m.get_string(3)
	_port = int(port_str) if port_str != "" else (443 if _scheme == "https" else 80)


## Versión asíncrona de fetch(): corre en un Thread interno y entrega el
## resultado en el hilo principal. Uso:
##   cavs.fetch_async("game_v2", func(result): print(result.ok))
func fetch_async(asset: String, on_done: Callable) -> void:
	var thread := Thread.new()
	_threads.append(thread)
	thread.start(func() -> void:
		var result := fetch(asset)
		_finish_async.call_deferred(thread, on_done, result))


func _finish_async(thread: Thread, on_done: Callable, result: Dictionary) -> void:
	thread.wait_to_finish()
	_threads.erase(thread)
	on_done.call(result)


## Descarga (o completa desde caché) todas las pistas de datos del asset.
## Devuelve: { ok: bool, error: String, files: Array[String],
##             bytes_wire: int, chunks_inline: int, refs: int }
func fetch(asset: String) -> Dictionary:
	var out := {"ok": false, "error": "", "files": [], "bytes_wire": 0,
			"chunks_inline": 0, "refs": 0}

	var mresp := _request("GET", "/api/assets/%s/manifest" % asset)
	if mresp.code != 200:
		out.error = "manifest HTTP %d" % mresp.code
		return out
	var manifest: Variant = JSON.parse_string(mresp.body.get_string_from_utf8())
	if manifest == null:
		out.error = "manifest JSON inválido"
		return out

	# Total lógico para la señal de progreso.
	var total_bytes := 0
	var done_bytes := 0
	for t: Dictionary in manifest.tracks:
		for c: Dictionary in t.init_chunks:
			total_bytes += int(c.len)
	for s: Dictionary in manifest.segments:
		for c: Dictionary in s.chunks:
			total_bytes += int(c.len)
	progress.emit(0, total_bytes, "manifest")

	# have-set: intersección de la caché local con la tabla de chunks firmada.
	DirAccess.make_dir_recursive_absolute(cache_dir + "/chunks")
	var have: Array[String] = []
	for hex: String in manifest.chunk_table:
		if FileAccess.file_exists(_chunk_path(hex)):
			have.append(hex)

	var sresp := _request("POST", "/api/assets/%s/sessions" % asset,
			JSON.stringify({"have": have}).to_utf8_buffer(),
			["Content-Type: application/json"])
	if sresp.code != 200:
		out.error = "session HTTP %d" % sresp.code
		return out
	var session: Variant = JSON.parse_string(sresp.body.get_string_from_utf8())
	var session_id: String = session.session_id

	# Batches: inits en el primero, segmentos en grupos.
	var track_ids: Array = []
	var segment_ids: Array = []
	for t: Dictionary in manifest.tracks:
		track_ids.append(int(t.track_id))
	for s: Dictionary in manifest.segments:
		segment_ids.append(int(s.segment_id))
	segment_ids.sort()

	# hash → longitud raw, para que las referencias también avancen la barra.
	var len_by_hash := {}
	for t: Dictionary in manifest.tracks:
		for c: Dictionary in t.init_chunks:
			len_by_hash[c.hash] = int(c.len)
	for s: Dictionary in manifest.segments:
		for c: Dictionary in s.chunks:
			len_by_hash[c.hash] = int(c.len)

	var first := true
	var i := 0
	while i < segment_ids.size() or first:
		var group := segment_ids.slice(i, i + BATCH_SEGMENTS)
		var req := {"track_inits": track_ids if first else [], "segment_ids": group}
		first = false
		i += BATCH_SEGMENTS
		var bresp := _request("POST", "/api/sessions/%s/batch" % session_id,
				JSON.stringify(req).to_utf8_buffer(),
				["Content-Type: application/json"])
		if bresp.code != 200:
			out.error = "batch HTTP %d" % bresp.code
			return out
		var applied := _apply_batch(bresp.body, out, len_by_hash)
		if applied.error != "":
			out.error = applied.error
			return out
		done_bytes += applied.bytes
		progress.emit(done_bytes, total_bytes, "download")

	# Reconstrucción por pista + verificación sha256 del manifiesto.
	var sha_by_name := {}
	for entry: Array in manifest.meta:
		if entry[0].begins_with("sha256:"):
			sha_by_name[entry[0].substr(7)] = entry[1]

	var fetch_dir := cache_dir + "/assets/" + asset
	DirAccess.make_dir_recursive_absolute(fetch_dir)
	for t: Dictionary in manifest.tracks:
		var segs: Array = []
		for s: Dictionary in manifest.segments:
			if int(s.track_id) == int(t.track_id):
				segs.append(s)
		segs.sort_custom(func(a, b): return int(a.pts_start) < int(b.pts_start))

		# Escritura streaming a temporal .part: verificar sha256 ANTES de que
		# exista el archivo final, y rename atómico al terminar. Un fetch
		# interrumpido nunca deja un pack corrupto con nombre válido.
		var name := (t.name as String).get_file()  # sin subdirectorios
		var path := fetch_dir + "/" + name
		var part_path := path + ".part"
		var file := FileAccess.open(part_path, FileAccess.WRITE)
		if file == null:
			out.error = "no puedo escribir " + part_path
			return out
		var hasher := HashingContext.new()
		hasher.start(HashingContext.HASH_SHA256)
		for chunk_ref: Dictionary in _init_and_segment_chunks(t, segs):
			var bytes := FileAccess.get_file_as_bytes(_chunk_path(chunk_ref.hash))
			if bytes.size() != int(chunk_ref.len):
				file.close()
				DirAccess.remove_absolute(part_path)
				out.error = "chunk %s ausente o corrupto en caché" % chunk_ref.hash
				return out
			file.store_buffer(bytes)
			hasher.update(bytes)
		file.close()

		var digest := hasher.finish().hex_encode()
		if sha_by_name.has(t.name):
			if digest != sha_by_name[t.name]:
				DirAccess.remove_absolute(part_path)
				out.error = "sha256 NO coincide para %s" % t.name
				return out
		elif require_sha256:
			push_warning("CAVS: el manifiesto no trae sha256 para %s" % t.name)
		var rename_err := DirAccess.rename_absolute(part_path, path)
		if rename_err != OK:
			out.error = "no puedo renombrar %s (err %d)" % [part_path, rename_err]
			return out
		out.files.append(path)
		progress.emit(done_bytes, total_bytes, "verify")

	progress.emit(total_bytes, total_bytes, "done")
	out.ok = true
	return out


## fetch() + ProjectSettings.load_resource_pack() del primer .pck del asset.
func ensure_pack(asset: String) -> bool:
	var result := fetch(asset)
	if not result.ok:
		push_error("CAVS fetch falló: " + result.error)
		return false
	for path: String in result.files:
		if path.ends_with(".pck") or path.ends_with(".zip"):
			return ProjectSettings.load_resource_pack(path)
	push_error("CAVS: el asset %s no contiene un .pck" % asset)
	return false


# --- protocolo CVSP -----------------------------------------------------------

## Procesa un batch binario: verifica longitudes, descomprime el wire y
## guarda los chunks inline en la caché.
## Devuelve { error: String, bytes: int } (bytes lógicos avanzados).
func _apply_batch(b: PackedByteArray, out: Dictionary, len_by_hash: Dictionary) -> Dictionary:
	var res := {"error": "", "bytes": 0}
	if b.size() < 13 or b.slice(0, 4) != "CVSP".to_ascii_buffer() or b[4] != 2:
		res.error = "batch CVSP inválido"
		return res
	var pos := 5
	for _section in 2:  # inits y segments comparten formato de instrucciones
		if pos + 4 > b.size():
			res.error = "batch truncado"
			return res
		var count := b.decode_u32(pos)
		pos += 4
		for _entry in count:
			# init: u32 track_id · segment: u64 segment_id — distinguimos por sección
			pos += 4 if _section == 0 else 8
			if pos + 4 > b.size():
				res.error = "batch truncado"
				return res
			var n := b.decode_u32(pos)
			pos += 4
			for _k in n:
				if pos + 33 > b.size():
					res.error = "batch truncado"
					return res
				var tag := b[pos]
				var hex := b.slice(pos + 1, pos + 33).hex_encode()
				pos += 33
				if tag == 0:
					out.refs += 1
					res.bytes += int(len_by_hash.get(hex, 0))
					continue
				if pos + 9 > b.size():
					res.error = "batch truncado"
					return res
				var compression := b[pos]
				var len_raw := b.decode_u32(pos + 1)
				var len_stored := b.decode_u32(pos + 5)
				pos += 9
				if pos + len_stored > b.size():
					res.error = "batch truncado"
					return res
				var payload := b.slice(pos, pos + len_stored)
				pos += len_stored
				out.bytes_wire += len_stored
				out.chunks_inline += 1
				res.bytes += len_raw

				var raw: PackedByteArray
				if compression == WIRE_ZSTD:
					raw = payload.decompress(len_raw, FileAccess.COMPRESSION_ZSTD)
				else:
					raw = payload
				if raw.size() != len_raw:
					res.error = "chunk %s: descompresión inválida" % hex
					return res
				var f := FileAccess.open(_chunk_path(hex), FileAccess.WRITE)
				if f == null:
					res.error = "no puedo escribir la caché"
					return res
				f.store_buffer(raw)
				f.close()
	return res


func _init_and_segment_chunks(track: Dictionary, segs: Array) -> Array:
	var chunks: Array = []
	for c in track.init_chunks:
		chunks.append(c)
	for s: Dictionary in segs:
		for c in s.chunks:
			chunks.append(c)
	return chunks


func _chunk_path(hex: String) -> String:
	return cache_dir + "/chunks/" + hex


# --- HTTP síncrono (HTTPClient; funciona headless y en hilos) -----------------

## Request con reintentos: errores de conexión, timeouts y 5xx se reintentan
## con backoff exponencial. Los batches son idempotentes por diseño (el
## servidor marca los chunks como conocidos al enviarlos, y las referencias
## se resuelven de la caché local), así que reintentar es seguro.
func _request(method_name: String, path: String,
		body := PackedByteArray(), headers: Array = []) -> Dictionary:
	var attempt := 0
	while true:
		var resp := _request_once(method_name, path, body, headers)
		if resp.code >= 200 and resp.code < 500:
			return resp
		if attempt >= max_retries:
			return resp
		var wait_ms := retry_base_ms * (1 << attempt)
		push_warning("CAVS: %s %s falló (HTTP %d), reintento %d/%d en %d ms" %
				[method_name, path, resp.code, attempt + 1, max_retries, wait_ms])
		OS.delay_msec(wait_ms)
		attempt += 1
	return {"code": 0, "body": PackedByteArray()}  # inalcanzable


func _request_once(method_name: String, path: String,
		body: PackedByteArray, headers: Array) -> Dictionary:
	var deadline := Time.get_ticks_msec() + request_timeout_ms
	var client := HTTPClient.new()
	var tls: TLSOptions = null
	if _scheme == "https":
		if ca_cert_path != "":
			tls = TLSOptions.client(X509Certificate.new())
			var cert := X509Certificate.new()
			if cert.load(ca_cert_path) == OK:
				tls = TLSOptions.client(cert)
		else:
			tls = TLSOptions.client()
	var err := client.connect_to_host(_host, _port, tls)
	if err != OK:
		return {"code": 0, "body": PackedByteArray()}
	while client.get_status() in [HTTPClient.STATUS_CONNECTING, HTTPClient.STATUS_RESOLVING]:
		if Time.get_ticks_msec() > deadline:
			return {"code": 0, "body": PackedByteArray()}
		client.poll()
		OS.delay_msec(2)
	if client.get_status() != HTTPClient.STATUS_CONNECTED:
		return {"code": 0, "body": PackedByteArray()}

	var method := HTTPClient.METHOD_GET if method_name == "GET" else HTTPClient.METHOD_POST
	var hdrs: PackedStringArray = PackedStringArray(headers)
	err = client.request_raw(method, path, hdrs, body)
	if err != OK:
		return {"code": 0, "body": PackedByteArray()}
	while client.get_status() == HTTPClient.STATUS_REQUESTING:
		if Time.get_ticks_msec() > deadline:
			return {"code": 0, "body": PackedByteArray()}
		client.poll()
		OS.delay_msec(2)
	if not client.has_response():
		return {"code": 0, "body": PackedByteArray()}

	var response := PackedByteArray()
	while client.get_status() == HTTPClient.STATUS_BODY:
		if Time.get_ticks_msec() > deadline:
			return {"code": 0, "body": PackedByteArray()}
		client.poll()
		var chunk := client.read_response_body_chunk()
		if chunk.size() == 0:
			OS.delay_msec(1)
		else:
			response.append_array(chunk)
	return {"code": client.get_response_code(), "body": response}
