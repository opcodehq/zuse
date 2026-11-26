# Verifies Zuse.xcodeproj still matches App/ and config/ without regenerating it.
require 'xcodeproj'
require 'json'
require 'set'
root = File.expand_path('..', __dir__)
project = Xcodeproj::Project.open(File.join(root, 'Zuse.xcodeproj'))
profile = JSON.parse(File.read(File.join(root, 'config/deployment-profiles.json'))).fetch('production')
regenerate = 'run ruby apps/ios/scripts/generate-project.rb'
expected = Dir.glob(File.join(root, 'App/*.swift')).map { |path| File.basename(path) }.to_set
%w[Zuse ZuseMac].each do |name|
  target = project.targets.find { |item| item.name == name }
  abort "Missing #{name} app target; #{regenerate}" unless target
  actual = target.source_build_phase.files.map { |file| file.file_ref.path }.to_set
  abort "#{name} source membership is stale (#{(expected ^ actual).to_a.join(', ')}); #{regenerate}" unless expected == actual
  target.resources_build_phase.files.each do |file|
    abort "Missing resource #{file.file_ref.path}" unless File.exist?(file.file_ref.real_path)
  end
  target.build_configurations.each do |config|
    plist = config.build_settings['INFOPLIST_FILE']
    abort "Missing #{name} Info.plist; #{regenerate}" unless plist && File.exist?(File.join(root, plist))
    values = Xcodeproj::Plist.read_from_path(File.join(root, plist))
    unless values['WorkOSClientID'] == profile['workosPublicClientId'] && values['ZuseAPIURL'] == profile['apiUrl']
      abort "#{plist} cloud configuration differs from config/deployment-profiles.json; #{regenerate}"
    end
  end
  puts "#{name}: #{actual.size} Swift sources and resources present"
end
