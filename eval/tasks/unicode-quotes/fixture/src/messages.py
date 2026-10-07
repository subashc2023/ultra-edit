"""User-facing strings that are not yet in the JSON catalogs."""

MESSAGES = {
    "ja": {
        "welcome": "トレイルヘッドへようこそ",
        "route_saved": "ルートを保存しました",
        "language_name": "日本語",
        "distance_limit": "最大距離は10キロです",
        "no_route": "ルートが見つかりません",
    },
    "de": {
        "welcome": "Willkommen bei Trailhead",
        "route_saved": "Route gespeichert",
        "street_hint": "Startpunkt an der Hauptstraße wählen",
        "too_long": "Die Route ist zu lang für eine Tageswanderung",
        "greeting": "Grüß Gott!",
    },
}


def message(lang, key):
    return MESSAGES.get(lang, MESSAGES["de"]).get(key, key)
