from pathlib import Path
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parents[3]
package = root / "packaging" / "windows" / "Package.wxs"
tree = ET.parse(package)
ns = {
    "w": "http://wixtoolset.org/schemas/v4/wxs",
    "ui": "http://wixtoolset.org/schemas/v4/wxs/ui",
}

ui = tree.find(".//ui:WixUI", ns)
assert ui is not None and ui.attrib["Id"] == "WixUI_FeatureTree"
assert ui.attrib["InstallDirectory"] == "INSTALLFOLDER"
major_upgrade = tree.find(".//w:MajorUpgrade", ns)
assert major_upgrade is not None
assert "AllowSameVersionUpgrades" not in major_upgrade.attrib
install_location = tree.find('.//w:RegistryValue[@Name="InstallLocation"]', ns)
assert install_location is not None and install_location.attrib["Value"] == "[INSTALLFOLDER]"
features = {item.attrib["Id"]: item for item in tree.findall(".//w:Feature", ns)}
assert features["PreserveConfiguration"].attrib["AllowAbsent"] == "yes"
assert features["PreserveData"].attrib["AllowAbsent"] == "yes"
actions = {item.attrib["Id"]: item for item in tree.findall(".//w:CustomAction", ns)}
assert actions["ResetOldConfiguration"].attrib["Return"] == "check"
assert actions["ResetOldData"].attrib["Return"] == "check"
sequence = {
    item.attrib["Action"]: item.attrib["Condition"]
    for item in tree.findall(".//w:InstallExecuteSequence/w:Custom", ns)
}
assert "&PreserveConfiguration <> 3" in sequence["ResetOldConfiguration"]
assert "&PreserveData <> 3" in sequence["ResetOldData"]
dialogs = package.read_text(encoding="utf-8")
assert "WixUI_FeatureTree" in dialogs
assert "Retain existing configuration" in dialogs
assert "Retain existing recording data" in dialogs
