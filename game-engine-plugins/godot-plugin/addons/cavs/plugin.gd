@tool
## Plugin de editor CAVS. El valor está en el runtime (CavsClient); esto solo
## registra el addon para activarlo en Proyecto > Ajustes > Plugins.
extends EditorPlugin


func _enter_tree() -> void:
	print("CAVS: plugin activo. Runtime: CavsClient (addons/cavs/cavs_client.gd)")


func _exit_tree() -> void:
	pass
