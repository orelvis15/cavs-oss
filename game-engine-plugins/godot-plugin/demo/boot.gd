## Demo visual de CAVS: instala un juego v1, luego actualiza a v2 y muestra
## cuántos bytes viajaron realmente frente al tamaño total del pack.
## Generado/reproducible con: game-engine-plugins/godot-plugin/demo/run_demo.sh
extends Control

const CavsClientScript := preload("res://addons/cavs/cavs_client.gd")

@export var server_url := "http://127.0.0.1:8991"

var _cavs: RefCounted
var _total_bytes := 0
var _bar: ProgressBar
var _stage_label: Label
var _stats: RichTextLabel
var _preview: TextureRect
var _btn_v1: Button
var _btn_v2: Button


func _ready() -> void:
	_build_ui()
	_log("[b]CAVS demo[/b] — servidor: %s" % server_url)
	_log("Pulsa [b]Instalar v1[/b] (descarga fría) y luego [b]Actualizar a v2[/b].")


func _build_ui() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)
	var panel := PanelContainer.new()
	panel.set_anchors_preset(Control.PRESET_FULL_RECT)
	add_child(panel)

	var margin := MarginContainer.new()
	for side in ["left", "right", "top", "bottom"]:
		margin.add_theme_constant_override("margin_" + side, 32)
	panel.add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 14)
	margin.add_child(root)

	var title := Label.new()
	title.text = "CAVS × Godot — updates que pesan lo que cambió"
	title.add_theme_font_size_override("font_size", 26)
	root.add_child(title)

	var buttons := HBoxContainer.new()
	buttons.add_theme_constant_override("separation", 10)
	root.add_child(buttons)
	_btn_v1 = Button.new()
	_btn_v1.text = "① Instalar v1 (frío)"
	_btn_v1.pressed.connect(func(): _fetch("game_v1"))
	buttons.add_child(_btn_v1)
	_btn_v2 = Button.new()
	_btn_v2.text = "② Actualizar a v2"
	_btn_v2.pressed.connect(func(): _fetch("game_v2"))
	buttons.add_child(_btn_v2)

	_stage_label = Label.new()
	_stage_label.text = "listo"
	root.add_child(_stage_label)

	_bar = ProgressBar.new()
	_bar.min_value = 0.0
	_bar.max_value = 100.0
	_bar.custom_minimum_size = Vector2(0, 26)
	root.add_child(_bar)

	_stats = RichTextLabel.new()
	_stats.bbcode_enabled = true
	_stats.fit_content = true
	_stats.custom_minimum_size = Vector2(0, 180)
	root.add_child(_stats)

	var preview_title := Label.new()
	preview_title.text = "Contenido montado desde el pack reconstruido:"
	root.add_child(preview_title)
	_preview = TextureRect.new()
	_preview.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	_preview.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_CENTERED
	_preview.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_preview.custom_minimum_size = Vector2(0, 200)
	root.add_child(_preview)


func _log(text: String) -> void:
	_stats.append_text(text + "\n")


func _fetch(asset: String) -> void:
	_btn_v1.disabled = true
	_btn_v2.disabled = true
	_total_bytes = 0
	_cavs = CavsClientScript.new(server_url)
	_cavs.progress.connect(_on_progress, CONNECT_DEFERRED)
	var t0 := Time.get_ticks_msec()
	_log("\n[b]%s[/b] — descargando..." % asset)
	_cavs.fetch_async(asset, func(result): _on_done(asset, result, t0))


func _on_progress(done: int, total: int, stage: String) -> void:
	_total_bytes = total
	_stage_label.text = "%s  ·  %.2f / %.2f MiB listos" % \
			[stage, done / 1048576.0, total / 1048576.0]
	_bar.value = 0.0 if total == 0 else done * 100.0 / total


func _on_done(asset: String, result: Dictionary, t0: int) -> void:
	_btn_v1.disabled = false
	_btn_v2.disabled = false
	if not result.ok:
		_log("[color=#f66]ERROR: %s[/color]" % result.error)
		return
	var secs := (Time.get_ticks_msec() - t0) / 1000.0
	var wire: float = result.bytes_wire / 1048576.0
	var total: float = _total_bytes / 1048576.0
	var saved := 0.0 if _total_bytes == 0 else \
			(1.0 - float(result.bytes_wire) / float(_total_bytes)) * 100.0
	_log("descargado: [b]%.2f MiB[/b] de %.2f MiB del pack  →  [b][color=#6f6]%.1f%% ahorrado[/color][/b]" %
			[wire, total, saved])
	_log("chunks nuevos: %d · reutilizados de caché: %d · %.1f s · sha256 verificado" %
			[result.chunks_inline, result.refs, secs])

	for path: String in result.files:
		if path.ends_with(".pck"):
			var mounted := ProjectSettings.load_resource_pack(path)
			_log("pack montado en runtime: %s" % ("sí" if mounted else "NO"))
	# Muestra contenido del pack: la textura nueva si existe (v2), si no la base.
	for tex_path in ["res://textures/t_new.png", "res://textures/t1.png"]:
		if ResourceLoader.exists(tex_path):
			_preview.texture = load(tex_path)
			_log("mostrando %s" % tex_path)
			break
