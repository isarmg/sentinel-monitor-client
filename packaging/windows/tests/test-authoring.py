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
assert ui is not None and ui.attrib["Id"] == "WixUI_InstallDir"
assert ui.attrib["InstallDirectory"] == "INSTALLFOLDER"
assert tree.find('.//w:Property[@Id="ARPNOMODIFY"]', ns) is None
major_upgrade = tree.find(".//w:MajorUpgrade", ns)
assert major_upgrade is not None
assert "AllowSameVersionUpgrades" not in major_upgrade.attrib
install_location = tree.find('.//w:RegistryValue[@Name="InstallLocation"]', ns)
assert install_location is not None and install_location.attrib["Value"] == "[INSTALLFOLDER]"
features = tree.findall(".//w:Feature", ns)
assert len(features) == 1
assert features[0].attrib["Id"] == "Complete"
assert features[0].attrib["AllowAbsent"] == "no"
assert not tree.findall(".//w:CustomAction", ns)
service_install = tree.find(".//w:ServiceInstall", ns)
assert service_install is not None
assert service_install.attrib["Name"] == "XcocClient"
assert service_install.attrib["Start"] == "auto"
assert service_install.attrib["Arguments"] == "--windows-service"
service_control = tree.find(".//w:ServiceControl", ns)
assert service_control is not None
assert service_control.attrib["Start"] == "install"
assert service_control.attrib["Stop"] == "both"
assert service_control.attrib["Remove"] == "uninstall"
assert not tree.findall('.//w:RegistryValue[@Key="Software\\Microsoft\\Windows\\CurrentVersion\\Run"]', ns)
